"""Linux backend for the Ferramari benchmark harness.

Linux keeps the private D-Bus session and the ``/proc`` reader the original
harness used.  The process reader takes CPU, RSS and thread counts from a single
``/proc/<pid>/stat`` read; ``status`` is only needed for optional PSS data.
"""

from __future__ import annotations

import ast
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Optional

from .common import (
    GpuSample,
    ProcInfo,
    boolean,
    read_json,
    run_command,
    wait_for,
)

DBUS_NAME = "io.github.ferramari.Ferramari.server"
OBJECT = "/io/github/ferramari/ferramari/server"
INTERFACE = "io.github.ferramari.ferramari.server"
CLK_TCK = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
PAGE_SIZE = os.sysconf("SC_PAGE_SIZE") if hasattr(os, "sysconf") else 4096


class LinuxControl:
    def __init__(self, address: str):
        self.address = address

    def _gdbus(self, *arguments: str, timeout: float = 10.0) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["DBUS_SESSION_BUS_ADDRESS"] = self.address
        return subprocess.run(
            ["gdbus", *arguments],
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )

    def _call(self, method: str, *arguments: str) -> subprocess.CompletedProcess[str]:
        return self._gdbus(
            "call",
            "--session",
            "--dest",
            DBUS_NAME,
            "--object-path",
            OBJECT,
            "--method",
            method,
            *arguments,
        )

    @staticmethod
    def _value(stdout: str) -> Any:
        text = stdout.strip()
        # `Properties.Get` returns a variant, so gdbus prints `(<...>,)` for
        # booleans and maps. Direct method calls omit the variant wrapper.
        if text.startswith("(") and text.endswith(",)"):
            text = text[1:-2]
        if text.startswith("<") and text.endswith(">"):
            text = text[1:-1]
        text = text.replace("true", "True").replace("false", "False")
        try:
            return ast.literal_eval(text)
        except (ValueError, SyntaxError):
            return None

    def wait_ready(self, timeout: float = 15.0) -> None:
        def ready() -> bool:
            result = self._gdbus(
                "call",
                "--session",
                "--dest",
                DBUS_NAME,
                "--object-path",
                OBJECT,
                "--method",
                "org.freedesktop.DBus.Peer.Ping",
            )
            return result.returncode == 0

        wait_for(ready, timeout, 0.1, "the Ferramari D-Bus name")

    def property(self, name: str) -> Any:
        result = self._call("org.freedesktop.DBus.Properties.Get", INTERFACE, name)
        if result.returncode != 0:
            return None
        return self._value(result.stdout)

    def snapshot(self) -> dict[str, Any]:
        snapshot: dict[str, Any] = {}
        for name in (
            "mode",
            "is_playing",
            "is_decoding",
            "is_paused_by_user",
            "hwdec_running",
            "primary_monitor",
            "monitor_states",
            "monitor_resolutions",
            "player_stats",
        ):
            value = self.property(name)
            if value is not None:
                snapshot[name] = value
        return snapshot

    def command(self, name: str, *args: Any) -> None:
        result = self._call(f"{INTERFACE}.{name}", *[str(arg) for arg in args])
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or f"command {name} failed")

    def property_set(self, name: str, value: Any) -> None:
        # The app exposes writable properties through the same D-Bus object.
        result = self._gdbus(
            "call",
            "--session",
            "--dest",
            DBUS_NAME,
            "--object-path",
            OBJECT,
            "--method",
            "org.freedesktop.DBus.Properties.Set",
            INTERFACE,
            name,
            str(value).lower() if isinstance(value, bool) else str(value),
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or f"property {name} failed")

    def quit(self) -> None:
        try:
            self.command("quit")
        except Exception:
            pass


class LinuxProcs:
    def __init__(self, with_pss: bool = True):
        self.with_pss = with_pss

    @staticmethod
    def _parse_stat(text: str) -> Optional[tuple[int, int, int, float, str]]:
        close = text.rfind(")")
        if close < 0:
            return None
        comm = text[text.find("(") + 1 : close]
        fields = text[close + 1 :].split()
        if len(fields) < 22:
            return None
        try:
            ppid = int(fields[1])
            utime = int(fields[11])
            stime = int(fields[12])
            threads = int(fields[17])
            rss_pages = int(fields[21])
        except ValueError:
            return None
        cpu = (utime + stime) / CLK_TCK
        return ppid, threads, rss_pages, cpu, comm

    @staticmethod
    def _pss(path: Path) -> Optional[int]:
        try:
            with (path / "smaps_rollup").open("r", encoding="ascii") as handle:
                for line in handle:
                    if line.startswith("Pss:"):
                        return int(line.split()[1]) * 1024
        except OSError:
            return None
        return None

    def _basic(self) -> dict[int, tuple[int, str, int]]:
        result: dict[int, tuple[int, str, int]] = {}
        try:
            entries = os.scandir("/proc")
        except OSError:
            return result
        with entries:
            for entry in entries:
                if not entry.name.isdigit():
                    continue
                try:
                    text = Path(entry.path, "stat").read_text(encoding="utf-8", errors="replace")
                except OSError:
                    continue
                parsed = self._parse_stat(text)
                if parsed is None:
                    continue
                pid = int(entry.name)
                result[pid] = (parsed[0], parsed[4], parsed[1])
        return result

    def sample_tree(
        self, roots: list[int], include_descendants: bool = True
    ) -> dict[int, ProcInfo]:
        basic = self._basic()
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
        result: dict[int, ProcInfo] = {}
        for pid in wanted:
            try:
                text = Path(f"/proc/{pid}/stat").read_text(
                    encoding="utf-8", errors="replace"
                )
            except OSError:
                continue
            parsed = self._parse_stat(text)
            if parsed is None:
                continue
            ppid, threads, rss_pages, cpu, comm = parsed
            info = ProcInfo(
                pid=pid,
                ppid=ppid,
                name=comm,
                cpu_seconds=cpu,
                rss_bytes=rss_pages * PAGE_SIZE,
                threads=threads,
            )
            if self.with_pss:
                info.pss_bytes = self._pss(Path(f"/proc/{pid}"))
            result[pid] = info
        return result

    def pids_of_name(self, name: str) -> list[int]:
        return sorted(
            pid for pid, (_, comm, _) in self._basic().items() if comm == name
        )


class LinuxGpu:
    def __init__(self) -> None:
        self._nvidia_at = 0.0
        self._nvidia_value: Optional[GpuSample] = None

    def sample(self) -> GpuSample:
        busy: list[float] = []
        vram: list[int] = []
        for path in Path("/sys/class/drm").glob("card*/device/gpu_busy_percent"):
            try:
                busy.append(float(path.read_text(encoding="ascii").strip()))
            except (OSError, ValueError):
                pass
        for path in Path("/sys/class/drm").glob("card*/device/mem_info_vram_used"):
            try:
                vram.append(int(path.read_text(encoding="ascii").strip()))
            except (OSError, ValueError):
                pass
        if busy or vram:
            return GpuSample(
                global_percent=max(busy) if busy else None,
                global_vram_bytes=max(vram) if vram else None,
                reason="" if busy else "AMDGPU utilization not exposed",
            )
        # NVIDIA does not expose utilization through sysfs.  A two-second cache
        # keeps a five-second nvidia-smi poll from dominating the sample loop.
        now = time.monotonic()
        if self._nvidia_value is not None and now - self._nvidia_at < 2.0:
            return self._nvidia_value
        output = run_command(
            [
                "nvidia-smi",
                "--query-gpu=utilization.gpu,memory.used",
                "--format=csv,noheader,nounits",
            ],
            timeout=5,
        )
        value = GpuSample(reason="no AMDGPU sysfs counter and nvidia-smi unavailable")
        if output:
            first = output.splitlines()[0]
            parts = [part.strip() for part in first.split(",")]
            if len(parts) >= 2:
                try:
                    value = GpuSample(
                        global_percent=float(parts[0]),
                        global_vram_bytes=int(float(parts[1])) * 1024 * 1024,
                    )
                except ValueError:
                    pass
        self._nvidia_at = now
        self._nvidia_value = value
        return value


def list_displays() -> list[dict[str, Any]]:
    output = run_command(["xrandr", "--listmonitors"], timeout=5)
    displays: list[dict[str, Any]] = []
    if not output:
        return displays
    pattern = re.compile(
        r"^\s*\d+:\s+[+*]*([^\s]+)\s+(\d+)/\d+x(\d+)/\d+\+(-?\d+)\+(-?\d+)"
    )
    for line in output.splitlines()[1:]:
        match = pattern.match(line)
        if not match:
            continue
        displays.append(
            {
                "name": match.group(1),
                "x": int(match.group(4)),
                "y": int(match.group(5)),
                "width": int(match.group(2)),
                "height": int(match.group(3)),
                "primary": "*" in line.split(":", 1)[1][:3],
            }
        )
    return displays


class LinuxEnvironment:
    def __init__(self, root: Path):
        self.root = root
        self.config_home = root / "config"
        self.cache_home = root / "cache"
        self.data_home = root / "data"
        self.state_home = root / "state"
        for directory in (
            self.config_home,
            self.cache_home,
            self.data_home,
            self.state_home,
        ):
            directory.mkdir(parents=True, exist_ok=True)
        (self.config_home / "ferramari").mkdir(parents=True, exist_ok=True)
        address, pid = self._start_dbus()
        self.address = address
        self.dbus_pid = pid

    def _start_dbus(self) -> tuple[str, int]:
        result = subprocess.run(
            [
                "dbus-daemon",
                "--session",
                "--fork",
                "--print-address=1",
                "--print-pid=1",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or "dbus-daemon failed")
        lines = result.stdout.splitlines()
        return lines[0], int(lines[1])

    def env(self) -> dict[str, str]:
        env = os.environ.copy()
        env.update(
            {
                "DBUS_SESSION_BUS_ADDRESS": self.address,
                "XDG_CONFIG_HOME": str(self.config_home),
                "XDG_CACHE_HOME": str(self.cache_home),
                "XDG_DATA_HOME": str(self.data_home),
                "XDG_STATE_HOME": str(self.state_home),
                "GSETTINGS_BACKEND": "memory",
                "GDK_BACKEND": "x11",
            }
        )
        return env

    def config_path(self) -> Path:
        return self.config_home / "ferramari" / "config.json"

    def log_path(self) -> Path:
        return self.root / "server.log"

    def control(self) -> LinuxControl:
        return LinuxControl(self.address)

    def close(self) -> None:
        try:
            os.kill(self.dbus_pid, 15)
        except OSError:
            pass


# Common backend vocabulary used by `scripts/benchmark.py`.
Procs = LinuxProcs
Gpu = LinuxGpu
Environment = LinuxEnvironment


def start_server(
    binary: Path, environment: dict[str, str], log_path: Path
) -> subprocess.Popen[Any]:
    log = log_path.open("w", encoding="utf-8", errors="replace")
    process = subprocess.Popen(
        [str(binary), "-b", "-d"],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=log,
        start_new_session=True,
    )
    process._ferramari_log = log  # type: ignore[attr-defined]
    return process


class CoverWindow:
    """Run the Linux cover-window helper built from the platform examples."""

    def __init__(self, binary: Path, monitor: int):
        self.binary = binary
        self.monitor = monitor
        self.process: Optional[subprocess.Popen[Any]] = None

    def start(self) -> None:
        if not self.binary.is_file():
            raise RuntimeError(
                f"Linux cover helper not built: {self.binary}; "
                "run `cargo build -p ferramari-platform --release "
                "--example cover_window`"
            )
        self.process = subprocess.Popen(
            [str(self.binary), "--monitor", str(self.monitor)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        time.sleep(1.0)

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

    def start(self) -> None:
        environment = dict(self.env)
        environment["FERRAMARI_UI_BOOT"] = f"page={self.page} size={self.size}"
        self.process = subprocess.Popen(
            [str(self.binary)],
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(3.0)

    def hide(self) -> None:
        # Linux closes the control surface instead of hiding it; the harness
        # only uses the hidden phase on Windows.
        pass

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
