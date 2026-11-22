"""Windows backend for the Ferramari benchmark harness.

The server has no D-Bus on Windows: it publishes a loopback JSON endpoint in
``%APPDATA%/ferramari/control-endpoint.json``.  Process counters come from the
Toolhelp snapshot and ``GetProcessTimes``/``GetProcessMemoryInfo``; GPU and
VRAM come from the same PDH counters Task Manager uses.  Nothing here is
imported on Linux.
"""

from __future__ import annotations

import ctypes
import json
import os
import re
import socket
import subprocess
import sys
import time
from ctypes import wintypes
from pathlib import Path
from typing import Any, Optional

from .common import (
    GpuSample,
    LogTail,
    ProcInfo,
    boolean,
    integer,
    number,
    read_json,
    run_command,
    wait_for,
)

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
psapi = ctypes.WinDLL("psapi", use_last_error=True)
pdh = ctypes.WinDLL("pdh", use_last_error=True)

TH32CS_SNAPPROCESS = 0x00000002
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
PROCESS_VM_READ = 0x0010
WM_CLOSE = 0x0010
CREATE_NO_WINDOW = 0x08000000
CREATE_NEW_PROCESS_GROUP = 0x00000200


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD),
        ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.c_size_t),
        ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", wintypes.LONG),
        ("dwFlags", wintypes.DWORD),
        ("szExeFile", wintypes.WCHAR * 260),
    ]


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
    ]


kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
kernel32.Process32FirstW.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(PROCESSENTRY32W),
]
kernel32.Process32FirstW.restype = wintypes.BOOL
kernel32.Process32NextW.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(PROCESSENTRY32W),
]
kernel32.Process32NextW.restype = wintypes.BOOL
kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.GetProcessTimes.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(wintypes.FILETIME),
    ctypes.POINTER(wintypes.FILETIME),
    ctypes.POINTER(wintypes.FILETIME),
    ctypes.POINTER(wintypes.FILETIME),
]
kernel32.GetProcessTimes.restype = wintypes.BOOL
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.CloseHandle.restype = wintypes.BOOL
psapi.GetProcessMemoryInfo.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(PROCESS_MEMORY_COUNTERS),
    wintypes.DWORD,
]
psapi.GetProcessMemoryInfo.restype = wintypes.BOOL

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.EnumWindows.argtypes = [ctypes.c_void_p, wintypes.LPARAM]
user32.EnumWindows.restype = wintypes.BOOL
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.IsWindowVisible.restype = wintypes.BOOL
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowTextW.restype = ctypes.c_int
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.PostMessageW.restype = wintypes.BOOL
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
user32.ShowWindow.restype = wintypes.BOOL

SW_MINIMIZE = 6
SW_RESTORE = 9

ENUM_WINDOWS_PROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)


def _filetime_seconds(value: wintypes.FILETIME) -> float:
    ticks = (int(value.dwHighDateTime) << 32) | int(value.dwLowDateTime)
    return ticks / 10_000_000.0


class WindowsProcs:
    def __init__(self) -> None:
        self._cached_basic: dict[int, tuple[int, str, int]] = {}

    def _basic_table(self) -> dict[int, tuple[int, str, int]]:
        snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
        if not snapshot or snapshot == INVALID_HANDLE_VALUE:
            return {}
        processes: dict[int, tuple[int, str, int]] = {}
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32W)
        try:
            if kernel32.Process32FirstW(snapshot, ctypes.byref(entry)):
                while True:
                    processes[int(entry.th32ProcessID)] = (
                        int(entry.th32ParentProcessID),
                        str(entry.szExeFile or ""),
                        int(entry.cntThreads),
                    )
                    if not kernel32.Process32NextW(snapshot, ctypes.byref(entry)):
                        break
        finally:
            kernel32.CloseHandle(snapshot)
        self._cached_basic = processes
        return processes

    def _details(self, pid: int, basic: tuple[int, str, int]) -> ProcInfo:
        _, name, threads = basic
        info = ProcInfo(pid=pid, ppid=basic[0], name=name, threads=threads)
        handle = kernel32.OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, False, pid
        )
        if not handle:
            handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not handle:
            return info
        try:
            created = wintypes.FILETIME()
            exited = wintypes.FILETIME()
            kernel = wintypes.FILETIME()
            user = wintypes.FILETIME()
            if kernel32.GetProcessTimes(
                handle,
                ctypes.byref(created),
                ctypes.byref(exited),
                ctypes.byref(kernel),
                ctypes.byref(user),
            ):
                info.cpu_seconds = _filetime_seconds(kernel) + _filetime_seconds(user)
            counters = PROCESS_MEMORY_COUNTERS()
            counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
            if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                info.rss_bytes = int(counters.WorkingSetSize)
        finally:
            kernel32.CloseHandle(handle)
        return info

    def sample_tree(
        self, roots: list[int], include_descendants: bool = True
    ) -> dict[int, ProcInfo]:
        basic = self._basic_table()
        if not basic:
            return {}
        if include_descendants:
            children: dict[int, list[int]] = {}
            for pid, (ppid, _, _) in basic.items():
                children.setdefault(ppid, []).append(pid)
            wanted: set[int] = set()
            stack = [pid for pid in roots if pid in basic]
            while stack:
                pid = stack.pop()
                if pid in wanted:
                    continue
                wanted.add(pid)
                stack.extend(children.get(pid, []))
        else:
            wanted = {pid for pid in roots if pid in basic}
        return {pid: self._details(pid, basic[pid]) for pid in wanted}

    def pids_of_name(self, name: str) -> list[int]:
        basic = self._basic_table()
        lowered = name.lower()
        return sorted(
            pid
            for pid, (_, executable, _) in basic.items()
            if executable.lower() == lowered
        )


def find_window(pid: int, title: Optional[str] = None) -> Optional[int]:
    found: list[int] = []

    @ENUM_WINDOWS_PROC
    def callback(hwnd: int, _lparam: int) -> bool:
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if int(owner.value) != pid or not user32.IsWindowVisible(hwnd):
            return True
        if title is not None:
            buffer = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, buffer, 256)
            if buffer.value != title:
                return True
        found.append(int(hwnd))
        return True

    user32.EnumWindows(callback, 0)
    return found[0] if found else None


def window_visible(hwnd: int) -> bool:
    return bool(user32.IsWindowVisible(hwnd))


def close_window(hwnd: int) -> None:
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)


def set_window_minimized(pid: int, minimized: bool) -> Optional[int]:
    """Minimize or restore a process's main window; return the handle used."""
    hwnd = find_window(pid)
    if hwnd:
        user32.ShowWindow(hwnd, SW_MINIMIZE if minimized else SW_RESTORE)
    return hwnd


def kill_tree(pid: int) -> None:
    subprocess.run(
        ["taskkill", "/PID", str(pid), "/T", "/F"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=10,
        creationflags=CREATE_NO_WINDOW,
    )


class WindowsEnvironment:
    """Isolated per-user directories for one measured server instance."""

    def __init__(self, root: Path):
        self.root = root
        self.app_data = root / "AppData" / "Roaming"
        self.local_app_data = root / "AppData" / "Local"
        self.user_profile = root / "User"
        self.videos = self.user_profile / "Videos"
        for directory in (
            self.app_data / "ferramari",
            self.local_app_data,
            self.user_profile,
            self.videos,
        ):
            directory.mkdir(parents=True, exist_ok=True)

    def env(self) -> dict[str, str]:
        environment = os.environ.copy()
        environment.update(
            {
                "APPDATA": str(self.app_data),
                "LOCALAPPDATA": str(self.local_app_data),
                "USERPROFILE": str(self.user_profile),
            }
        )
        return environment

    def config_path(self) -> Path:
        return self.app_data / "ferramari" / "config.json"

    def log_path(self) -> Path:
        return self.root / "server.log"

    def control(self) -> "WindowsControl":
        return WindowsControl(self.app_data)

    def close(self) -> None:
        # The endpoint file belongs to the throwaway root and disappears with
        # it; there is no global resource to release here.
        pass


class WindowsControl:
    def __init__(self, app_data: Path):
        self.endpoint_path = app_data / "ferramari" / "control-endpoint.json"
        self.endpoint: Optional[dict[str, Any]] = None

    def _request(self, method: str, params: Any = None) -> Any:
        endpoint = self.endpoint or read_json(self.endpoint_path)
        if not endpoint:
            raise RuntimeError("control-endpoint.json is not published")
        request = json.dumps(
            {
                "token": endpoint.get("token"),
                "method": method,
                "params": params,
            },
            ensure_ascii=False,
        )
        with socket.create_connection(("127.0.0.1", int(endpoint["port"])), timeout=5) as stream:
            stream.sendall((request + "\n").encode("utf-8"))
            stream.settimeout(10)
            data = bytearray()
            while not data.endswith(b"\n"):
                chunk = stream.recv(65536)
                if not chunk:
                    break
                data.extend(chunk)
        reply = json.loads(data.decode("utf-8"))
        if not reply.get("ok"):
            raise RuntimeError(reply.get("error") or "control request failed")
        return reply.get("result")

    def wait_ready(self, timeout: float = 15.0) -> None:
        def ready() -> bool:
            endpoint = read_json(self.endpoint_path)
            if not endpoint:
                return False
            self.endpoint = endpoint
            try:
                result = self._request("ping")
            except Exception:
                return False
            return bool(result and result.get("pid"))

        wait_for(ready, timeout, 0.1, "the Windows control endpoint")
        self.endpoint = read_json(self.endpoint_path)

    def ping(self) -> Optional[dict[str, Any]]:
        return self._request("ping")

    def snapshot(self) -> dict[str, Any]:
        result = self._request("snapshot")
        return result if isinstance(result, dict) else {}

    def command(self, name: str, *args: Any) -> None:
        self._request("command", {"name": name, "args": list(args)})

    def property_set(self, name: str, value: Any) -> None:
        self._request("property_set", {"name": name, "value": value})

    def quit(self) -> None:
        try:
            self.command("quit")
        except Exception:
            pass


class PdhQuery:
    """One PDH query with optional GPU-engine and memory counters."""

    PDH_FMT_DOUBLE = 0x00000200
    PDH_FMT_NOCAP100 = 0x00008000
    PDH_MORE_DATA = 0x800007D2

    def __init__(self) -> None:
        self.query = ctypes.c_void_p()
        self.engine = ctypes.c_void_p()
        self.adapter_memory = ctypes.c_void_p()
        self.process_memory = ctypes.c_void_p()
        self.opened = False
        self.primed = False
        self.notes: list[str] = []
        self._open()

    def _open(self) -> None:
        if pdh.PdhOpenQueryW(None, 0, ctypes.byref(self.query)) != 0:
            self.notes.append("PdhOpenQuery failed")
            return
        for attribute, path in (
            ("engine", r"\GPU Engine(*)\Utilization Percentage"),
            ("adapter_memory", r"\GPU Adapter Memory(*)\Dedicated Usage"),
            ("process_memory", r"\GPU Process Memory(*)\Dedicated Usage"),
        ):
            counter = ctypes.c_void_p()
            status = pdh.PdhAddEnglishCounterW(
                self.query, path, 0, ctypes.byref(counter)
            )
            if status == 0:
                setattr(self, attribute, counter)
            else:
                self.notes.append(f"counter unavailable: {path}")
        self.opened = True

    def __del__(self) -> None:
        if getattr(self, "opened", False) and self.query:
            pdh.PdhCloseQuery(self.query)

    def _array(self, counter: ctypes.c_void_p, fmt: int) -> list[tuple[str, float]]:
        if not counter:
            return []
        size = wintypes.DWORD(0)
        count = wintypes.DWORD(0)
        status = pdh.PdhGetFormattedCounterArrayW(
            counter, fmt, ctypes.byref(size), ctypes.byref(count), None
        )
        if status not in (0, self.PDH_MORE_DATA) or size.value == 0:
            return []
        buffer = ctypes.create_string_buffer(size.value)
        items = ctypes.cast(buffer, ctypes.POINTER(PDH_FMT_COUNTERVALUE_ITEM_W))
        status = pdh.PdhGetFormattedCounterArrayW(
            counter, fmt, ctypes.byref(size), ctypes.byref(count), items
        )
        if status != 0:
            return []
        values: list[tuple[str, float]] = []
        for index in range(count.value):
            item = items[index]
            name = item.szName
            if not name:
                continue
            if int(item.FmtValue.CStatus) > 1:
                continue
            value = float(item.FmtValue.doubleValue)
            if value >= 0.0:
                values.append((str(name), value))
        return values

    def sample(self) -> GpuSample:
        if not self.opened or not self.query:
            return GpuSample(reason="; ".join(self.notes) or "PDH unavailable")
        if pdh.PdhCollectQueryData(self.query) != 0:
            return GpuSample(reason="PdhCollectQueryData failed")
        if not self.primed:
            self.primed = True
            return GpuSample(reason="first PDH sample")
        fmt = self.PDH_FMT_DOUBLE | self.PDH_FMT_NOCAP100
        per_pid: dict[int, float] = {}
        per_pid_luid: dict[int, str] = {}
        global_engines: dict[str, float] = {}
        for name, value in self._array(self.engine, fmt):
            match = re.match(
                r"pid_(\d+)_(luid_0x[0-9A-Fa-f]+_0x[0-9A-Fa-f]+)_(.+)", name
            )
            if not match:
                continue
            pid = int(match.group(1))
            luid = match.group(2)
            engine = match.group(3)
            global_engines[engine] = global_engines.get(engine, 0.0) + value
            per_pid[pid] = max(per_pid.get(pid, 0.0), value)
            per_pid_luid.setdefault(pid, luid)
        global_percent = (
            min(100.0, max(global_engines.values())) if global_engines else None
        )
        adapter_memory: list[float] = []
        for _, value in self._array(self.adapter_memory, fmt):
            adapter_memory.append(value)
        global_vram = (
            int(max(adapter_memory)) if adapter_memory else None
        )
        per_pid_vram: dict[int, int] = {}
        for name, value in self._array(self.process_memory, fmt):
            match = re.match(r"pid_(\d+)_", name)
            if not match:
                continue
            pid = int(match.group(1))
            per_pid_vram[pid] = per_pid_vram.get(pid, 0) + int(value)
        return GpuSample(
            global_percent=global_percent,
            global_vram_bytes=global_vram,
            per_pid_percent=per_pid,
            per_pid_vram_bytes=per_pid_vram,
            per_pid_luid=per_pid_luid,
            reason="; ".join(self.notes),
        )


# PDH_FMT_COUNTERVALUE is a union.  The double member is at offset 8 on x64
# because the union starts at an 8-byte boundary after the DWORD status.
class PDH_FMT_COUNTERVALUE(ctypes.Structure):
    _fields_ = [
        ("CStatus", wintypes.DWORD),
        ("_padding", wintypes.DWORD),
        ("doubleValue", ctypes.c_double),
    ]


class PDH_FMT_COUNTERVALUE_ITEM_W(ctypes.Structure):
    _fields_ = [
        ("szName", wintypes.LPWSTR),
        ("FmtValue", PDH_FMT_COUNTERVALUE),
    ]


pdh.PdhOpenQueryW.argtypes = [
    wintypes.LPCWSTR,
    ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_void_p),
]
pdh.PdhOpenQueryW.restype = wintypes.ULONG
pdh.PdhAddEnglishCounterW.argtypes = [
    ctypes.c_void_p,
    wintypes.LPCWSTR,
    ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_void_p),
]
pdh.PdhAddEnglishCounterW.restype = wintypes.ULONG
pdh.PdhCollectQueryData.argtypes = [ctypes.c_void_p]
pdh.PdhCollectQueryData.restype = wintypes.ULONG
pdh.PdhGetFormattedCounterArrayW.argtypes = [
    ctypes.c_void_p,
    wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(PDH_FMT_COUNTERVALUE_ITEM_W),
]
pdh.PdhGetFormattedCounterArrayW.restype = wintypes.ULONG
pdh.PdhCloseQuery.argtypes = [ctypes.c_void_p]
pdh.PdhCloseQuery.restype = wintypes.ULONG


def list_displays() -> list[dict[str, Any]]:
    script = (
        "Add-Type -AssemblyName System.Windows.Forms; "
        "[System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "
        "'{0}|{1}|{2}|{3}|{4}|{5}' -f $_.DeviceName,$_.Bounds.X,$_.Bounds.Y,"
        "$_.Bounds.Width,$_.Bounds.Height,$_.Primary }"
    )
    output = run_command(
        ["powershell", "-NoProfile", "-Command", script], timeout=15
    )
    displays: list[dict[str, Any]] = []
    if not output:
        return displays
    for line in output.splitlines():
        parts = line.strip().split("|")
        if len(parts) != 6:
            continue
        try:
            displays.append(
                {
                    "name": parts[0],
                    "x": int(parts[1]),
                    "y": int(parts[2]),
                    "width": int(parts[3]),
                    "height": int(parts[4]),
                    "primary": parts[5].lower() == "true",
                }
            )
        except ValueError:
            continue
    return displays


class CoverWindow:
    def __init__(self, script: Path, monitor: int):
        self.process: Optional[subprocess.Popen[Any]] = None
        self.script = script
        self.monitor = monitor

    def start(self) -> None:
        self.process = subprocess.Popen(
            [
                "powershell",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(self.script),
                "-Monitor",
                str(self.monitor),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=CREATE_NO_WINDOW,
        )
        wait_for(
            lambda: self.process and find_window(self.process.pid) is not None,
            10,
            0.1,
            "the Windows cover window",
        )

    def stop(self) -> None:
        if not self.process:
            return
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.process = None


class UiProcess:
    def __init__(self, binary: Path, env: dict[str, str], page: str, size: str):
        self.binary = binary
        self.env = env
        self.page = page
        self.size = size
        self.process: Optional[subprocess.Popen[Any]] = None
        self.hwnd: Optional[int] = None

    def start(self) -> None:
        environment = dict(self.env)
        environment["FERRAMARI_UI_BOOT"] = f"page={self.page} size={self.size}"
        self.process = subprocess.Popen(
            [str(self.binary)],
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.hwnd = wait_for(
            lambda: find_window(self.process.pid, "Ferramari")
            if self.process
            else None,
            20,
            0.1,
            "the Ferramari control window",
        )
        time.sleep(2.0)

    def hide(self) -> None:
        if self.hwnd:
            close_window(self.hwnd)
            wait_for(
                lambda: not window_visible(self.hwnd) if self.hwnd else True,
                10,
                0.1,
                "the control window to hide",
            )

    def stop(self) -> None:
        if not self.process:
            return
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.process = None


# Common backend vocabulary used by `scripts/benchmark.py`.
Procs = WindowsProcs
Gpu = PdhQuery
Environment = WindowsEnvironment


def start_server(
    binary: Path,
    environment: dict[str, str],
    log_path: Path,
) -> subprocess.Popen[Any]:
    log = log_path.open("w", encoding="utf-8", errors="replace")
    process = subprocess.Popen(
        [str(binary), "-b", "-d"],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=log,
        creationflags=CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
    )
    process._ferramari_log = log  # type: ignore[attr-defined]
    return process
