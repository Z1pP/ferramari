"""Shared pieces of the Ferramari benchmark harness.

The harness deliberately uses only the Python standard library.  Every value
that was not measured is stored as ``None`` (``null`` in JSON, an empty cell in
CSV) together with a reason, so a missing GPU counter can never look like an
idle GPU.
"""

from __future__ import annotations

import csv
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable, Optional

SCHEMA_VERSION = 2
MIB = 1024 * 1024
GIB = 1024 * MIB


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def read_json(path: Path, default: Any = None) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return default


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def sha256_file(path: Path, limit: Optional[int] = None) -> Optional[str]:
    """Hash a source file.  Large videos are hashed completely once per run."""
    try:
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            remaining = limit
            while True:
                chunk = handle.read(1024 * 1024)
                if not chunk:
                    break
                if remaining is not None:
                    chunk = chunk[:remaining]
                    remaining -= len(chunk)
                digest.update(chunk)
                if remaining is not None and remaining <= 0:
                    break
        return digest.hexdigest()
    except OSError:
        return None


def percentile(values: list[float], fraction: float) -> Optional[float]:
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * fraction
    low = math.floor(position)
    high = math.ceil(position)
    if low == high:
        return ordered[low]
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def summarize(values: Iterable[Optional[float]]) -> dict[str, Any]:
    present = [float(value) for value in values if value is not None]
    if not present:
        return {
            "median": None,
            "mean": None,
            "p95": None,
            "min": None,
            "max": None,
            "samples": 0,
        }
    return {
        "median": statistics.median(present),
        "mean": statistics.fmean(present),
        "p95": percentile(present, 0.95),
        "min": min(present),
        "max": max(present),
        "samples": len(present),
    }


def number(value: Any) -> Optional[float]:
    """Convert a bus string/number to a float, or ``None`` when unknown."""
    if value is None or value == "":
        return None
    if isinstance(value, bool):
        return 1.0 if value else 0.0
    try:
        result = float(value)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(result):
        return None
    return result


def integer(value: Any) -> Optional[int]:
    parsed = number(value)
    return None if parsed is None else int(parsed)


def boolean(value: Any) -> Optional[bool]:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        lowered = value.strip().lower()
        if lowered == "true":
            return True
        if lowered == "false":
            return False
    return None


def wait_for(
    predicate: Callable[[], Any],
    timeout: float,
    interval: float = 0.1,
    description: str = "condition",
) -> Any:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = predicate()
        except Exception:  # the API may be between restarts
            result = None
        if result:
            return result
        time.sleep(interval)
    raise TimeoutError(f"timed out after {timeout:.1f}s waiting for {description}")


@dataclass
class ProcInfo:
    pid: int
    ppid: int
    name: str
    cpu_seconds: Optional[float] = None
    rss_bytes: Optional[int] = None
    pss_bytes: Optional[int] = None
    threads: Optional[int] = None


@dataclass
class GpuSample:
    global_percent: Optional[float] = None
    global_vram_bytes: Optional[int] = None
    per_pid_percent: dict[int, float] = None  # type: ignore[assignment]
    per_pid_vram_bytes: dict[int, int] = None  # type: ignore[assignment]
    per_pid_luid: dict[int, str] = None  # type: ignore[assignment]
    reason: str = ""

    def __post_init__(self) -> None:
        if self.per_pid_percent is None:
            self.per_pid_percent = {}
        if self.per_pid_vram_bytes is None:
            self.per_pid_vram_bytes = {}
        if self.per_pid_luid is None:
            self.per_pid_luid = {}


def group_metrics(
    previous: dict[int, ProcInfo],
    current: dict[int, ProcInfo],
    elapsed: float,
    groups: dict[str, set[int]],
    gpu: Optional[GpuSample],
    logical_cpus: int,
) -> dict[str, dict[str, Any]]:
    """Aggregate one sample per named PID group.

    CPU is a delta between two samples.  A group containing a process that was
    not present in the previous sample reports ``cpu_pct = None`` and
    ``cpu_partial = True`` instead of inventing a smaller rate.
    """
    result: dict[str, dict[str, Any]] = {}
    for group, pids in groups.items():
        live = {pid for pid in pids if pid in current}
        rss_values = [
            current[pid].rss_bytes
            for pid in live
            if current[pid].rss_bytes is not None
        ]
        pss_values = [
            current[pid].pss_bytes
            for pid in live
            if current[pid].pss_bytes is not None
        ]
        thread_values = [
            current[pid].threads
            for pid in live
            if current[pid].threads is not None
        ]
        common = live & set(previous)
        cpu_delta: Optional[float] = None
        if common and elapsed > 0:
            deltas = [
                current[pid].cpu_seconds - previous[pid].cpu_seconds
                for pid in common
                if current[pid].cpu_seconds is not None
                and previous[pid].cpu_seconds is not None
            ]
            if deltas:
                cpu_delta = sum(deltas)
        cpu_pct = (
            cpu_delta / elapsed * 100.0 if cpu_delta is not None and elapsed > 0 else None
        )
        cpu_machine = (
            cpu_pct / logical_cpus
            if cpu_pct is not None and logical_cpus > 0
            else None
        )
        per_pid_gpu = (
            {
                pid: gpu.per_pid_percent[pid]
                for pid in live
                if pid in gpu.per_pid_percent
            }
            if gpu
            else {}
        )
        per_pid_vram = (
            {
                pid: gpu.per_pid_vram_bytes[pid]
                for pid in live
                if pid in gpu.per_pid_vram_bytes
            }
            if gpu
            else {}
        )
        result[group] = {
            "processes": len(live),
            "cpu_pct": cpu_pct,
            "cpu_pct_machine": cpu_machine,
            "cpu_partial": bool(live - common) or bool(set(previous) & pids - live),
            "rss_mib": sum(rss_values) / MIB if rss_values else None,
            "rss_complete": len(rss_values) == len(live),
            "pss_mib": sum(pss_values) / MIB if pss_values else None,
            "pss_complete": len(pss_values) == len(live),
            "threads": sum(thread_values) if thread_values else None,
            "threads_complete": len(thread_values) == len(live),
            "gpu_pct": max(per_pid_gpu.values()) if per_pid_gpu else None,
            "vram_mib": sum(per_pid_vram.values()) / MIB if per_pid_vram else None,
            "pids": sorted(live),
        }
    return result


def playback_from_snapshot(snapshot: dict[str, Any]) -> dict[str, Any]:
    stats = snapshot.get("player_stats") or {}
    if not isinstance(stats, dict):
        stats = {}
    fps = number(stats.get("decoded_fps"))
    if fps is None:
        fps = number(stats.get("fps"))
    return {
        "is_playing": boolean(snapshot.get("is_playing")),
        "is_decoding": boolean(snapshot.get("is_decoding")),
        "is_paused_by_user": boolean(snapshot.get("is_paused_by_user")),
        "mode": snapshot.get("mode"),
        "hwdec_running": snapshot.get("hwdec_running") or None,
        "fps": fps,
        "display_fps": number(stats.get("display_fps")),
        "dropped": integer(stats.get("dropped")),
        "position": number(stats.get("position")),
        "duration": number(stats.get("duration")),
        "width": integer(stats.get("width")),
        "height": integer(stats.get("height")),
        "active": boolean(stats.get("active")),
        "monitor_states": snapshot.get("monitor_states") or {},
        "monitor_resolutions": snapshot.get("monitor_resolutions") or {},
        "primary_monitor": snapshot.get("primary_monitor") or None,
    }


def first_frame_reached(playback: dict[str, Any]) -> bool:
    """``is_decoding`` is the cold-start flag; false means a frame landed."""
    return bool(playback.get("is_playing")) and playback.get("is_decoding") is False


class LogTail:
    def __init__(self, path: Path):
        self.path = path
        self.offset = 0
        self.pending = ""

    def read(self) -> list[str]:
        try:
            with self.path.open("r", encoding="utf-8", errors="replace") as handle:
                handle.seek(self.offset)
                chunk = handle.read()
                self.offset = handle.tell()
        except OSError:
            return []
        parts = (self.pending + chunk).split("\n")
        self.pending = parts.pop()
        return parts

    def wait_marker(self, marker: str, timeout: float) -> Optional[str]:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for line in self.read():
                if marker in line:
                    return line
            time.sleep(0.1)
        return None


def run_command(
    arguments: list[str], timeout: float = 10.0, env: Optional[dict[str, str]] = None
) -> Optional[str]:
    try:
        result = subprocess.run(
            arguments,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            timeout=timeout,
        )
        return result.stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return None


def resolve_tool(program: str) -> Optional[str]:
    """Find a media tool the same way Ferramari's ``paths::tool`` does."""
    import shutil

    found = shutil.which(program)
    if found:
        return found
    if sys.platform == "win32":
        candidates = []
        program_files = os.environ.get("ProgramFiles")
        local_app_data = os.environ.get("LOCALAPPDATA")
        if program_files:
            candidates.append(Path(program_files) / "MPV Player" / f"{program}.exe")
            candidates.append(Path(program_files) / program / f"{program}.exe")
        if local_app_data:
            candidates.append(
                Path(local_app_data) / "Microsoft" / "WinGet" / "Links" / f"{program}.exe"
            )
            candidates.append(
                Path(local_app_data) / "Programs" / program / f"{program}.exe"
            )
        for candidate in candidates:
            if candidate.is_file():
                return str(candidate)
    return None


def tool_version(program: str) -> Optional[str]:
    """First line of ``--version`` (or ``-version`` for ffmpeg)."""
    executable = resolve_tool(program) or program
    for flag in ("--version", "-version"):
        output = run_command([executable, flag], timeout=10)
        if output:
            return output.splitlines()[0].strip()
    return None


def binary_version(binary: Path) -> Optional[str]:
    output = run_command([str(binary), "--version"], timeout=10)
    return output.splitlines()[0].strip() if output else None


def git_commit(directory: Path) -> Optional[str]:
    output = run_command(
        ["git", "-C", str(directory), "rev-parse", "HEAD"], timeout=5
    )
    return output.strip() if output else None


def gather_environment(
    binary: Path,
    backend_name: str,
    displays: list[dict[str, Any]],
    extra: Optional[dict[str, Any]] = None,
) -> dict[str, Any]:
    cpu_name = platform.processor() or None
    try:
        if sys.platform == "win32":
            import winreg

            with winreg.OpenKey(
                winreg.HKEY_LOCAL_MACHINE,
                r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            ) as key:
                cpu_name = winreg.QueryValueEx(key, "ProcessorNameString")[0].strip()
    except Exception:
        pass
    memory_total = total_memory_bytes()
    environment = {
        "platform": backend_name,
        "system": platform.system(),
        "release": platform.release(),
        "version": platform.version(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "logical_cpus": os.cpu_count(),
        "cpu": cpu_name,
        "memory_total_mib": memory_total / MIB if memory_total else None,
        "gpus": list_gpus(),
        "displays": displays,
        "binary": str(binary),
        "binary_version": binary_version(binary),
        "git_commit": git_commit(binary.parent.parent.parent),
        "mpv": tool_version("mpv"),
        "ffmpeg": tool_version("ffmpeg"),
        "ffprobe": tool_version("ffprobe"),
        "yt_dlp": tool_version("yt-dlp"),
        "sampled_at": utc_now(),
    }
    if extra:
        environment.update(extra)
    return environment


def list_gpus() -> list[dict[str, Any]]:
    """Adapter names and driver versions, without a new Python dependency."""
    if sys.platform == "win32":
        output = run_command(
            [
                "powershell",
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_VideoController | ForEach-Object { "
                "'{0}|{1}' -f $_.Name,$_.DriverVersion }",
            ],
            timeout=15,
        )
        result = []
        for line in (output or "").splitlines():
            parts = line.strip().split("|")
            if len(parts) == 2:
                result.append({"name": parts[0], "driver": parts[1]})
        return result
    output = run_command(["lspci"], timeout=5)
    if not output:
        return []
    return [
        {"name": line.split(": ", 1)[-1], "driver": None}
        for line in output.splitlines()
        if "VGA compatible controller" in line or "3D controller" in line
    ]


def total_memory_bytes() -> Optional[int]:
    try:
        if sys.platform == "win32":
            import ctypes
            from ctypes import wintypes

            class MEMORYSTATUSEX(ctypes.Structure):
                _fields_ = [
                    ("dwLength", wintypes.DWORD),
                    ("dwMemoryLoad", wintypes.DWORD),
                    ("ullTotalPhys", ctypes.c_ulonglong),
                    ("ullAvailPhys", ctypes.c_ulonglong),
                    ("ullTotalPageFile", ctypes.c_ulonglong),
                    ("ullAvailPageFile", ctypes.c_ulonglong),
                    ("ullTotalVirtual", ctypes.c_ulonglong),
                    ("ullAvailVirtual", ctypes.c_ulonglong),
                    ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
                ]

            status = MEMORYSTATUSEX()
            status.dwLength = ctypes.sizeof(MEMORYSTATUSEX)
            if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
                return int(status.ullTotalPhys)
            return None
        page_size = os.sysconf("SC_PAGE_SIZE")
        with open("/proc/meminfo", encoding="ascii") as handle:
            for line in handle:
                if line.startswith("MemTotal:"):
                    return int(line.split()[1]) * 1024
        return None
    except Exception:
        return None


def sample_to_row(phase: str, index: int, sample: dict[str, Any]) -> dict[str, Any]:
    row: dict[str, Any] = {
        "phase": phase,
        "sample": index,
        "t": sample["t"],
    }
    playback = sample.get("playback") or {}
    for key in (
        "is_playing",
        "is_decoding",
        "fps",
        "dropped",
        "position",
        "active",
        "hwdec_running",
        "primary_monitor",
    ):
        row[key] = playback.get(key)
    row["monitor_states"] = json.dumps(
        playback.get("monitor_states") or {}, ensure_ascii=False, sort_keys=True
    )
    for group, metrics in (sample.get("groups") or {}).items():
        for key in (
            "cpu_pct",
            "cpu_pct_machine",
            "rss_mib",
            "pss_mib",
            "threads",
            "processes",
            "gpu_pct",
            "vram_mib",
        ):
            row[f"{group}_{key}"] = metrics.get(key)
    row["gpu_global_pct"] = sample.get("gpu_global_pct")
    row["vram_global_mib"] = sample.get("vram_global_mib")
    row["mpv_pids"] = ",".join(str(pid) for pid in sample.get("mpv_pids") or [])
    row["notes"] = "; ".join(sample.get("notes") or [])
    return row


CSV_COLUMNS = [
    "phase",
    "sample",
    "t",
    "is_playing",
    "is_decoding",
    "fps",
    "dropped",
    "position",
    "active",
    "hwdec_running",
    "primary_monitor",
    "monitor_states",
    "gpu_global_pct",
    "vram_global_mib",
    "mpv_pids",
    "notes",
]


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    columns = list(CSV_COLUMNS)
    for row in rows:
        for key in row:
            if key not in columns:
                columns.append(key)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns)
        writer.writeheader()
        for row in rows:
            writer.writerow(
                {
                    key: ("" if row.get(key) is None else row.get(key))
                    for key in columns
                }
            )


def summarize_samples(samples: list[dict[str, Any]]) -> dict[str, Any]:
    """Median/p95 per phase, group and metric; ``samples == 0`` when absent."""
    phases: dict[str, list[dict[str, Any]]] = {}
    for sample in samples:
        phases.setdefault(sample["phase"], []).append(sample)
    summary: dict[str, Any] = {}
    for phase, phase_samples in phases.items():
        groups: dict[str, Any] = {}
        group_names = sorted(
            {
                group
                for sample in phase_samples
                for group in (sample.get("groups") or {})
            }
        )
        for group in group_names:
            metrics: dict[str, Any] = {}
            for key in (
                "cpu_pct",
                "cpu_pct_machine",
                "rss_mib",
                "pss_mib",
                "threads",
                "processes",
                "gpu_pct",
                "vram_mib",
            ):
                metrics[key] = summarize(
                    (sample.get("groups") or {}).get(group, {}).get(key)
                    for sample in phase_samples
                )
            groups[group] = metrics
        playback: dict[str, Any] = {}
        for key in ("fps", "dropped", "position", "gpu_global_pct", "vram_global_mib"):
            if key in ("gpu_global_pct", "vram_global_mib"):
                values = [sample.get(key) for sample in phase_samples]
            else:
                values = [
                    (sample.get("playback") or {}).get(key) for sample in phase_samples
                ]
            playback[key] = summarize(values)
        first_t = phase_samples[0].get("wall_t", phase_samples[0]["t"])
        last_t = phase_samples[-1].get("wall_t", phase_samples[-1]["t"])
        summary[phase] = {
            "samples": len(phase_samples),
            "duration_s": last_t - first_t if len(phase_samples) > 1 else 0,
            "groups": groups,
            "playback": playback,
        }
    return summary


def _slope_per_hour(points: list[tuple[float, float]]) -> Optional[float]:
    if len(points) < 2:
        return None
    mean_x = sum(x for x, _ in points) / len(points)
    mean_y = sum(y for _, y in points) / len(points)
    denominator = sum((x - mean_x) ** 2 for x, _ in points)
    if denominator == 0.0:
        return None
    numerator = sum((x - mean_x) * (y - mean_y) for x, y in points)
    return numerator / denominator * 3600.0


def soak_summary(
    samples: list[dict[str, Any]],
    log_lines: list[str],
    planned_duration_s: float,
) -> dict[str, Any]:
    """Leak/recovery view over a long-run sample stream.

    The individual phase samples are still available in JSONL; this is the
    compact answer a 24-hour report needs: where memory/threads went, whether
    the player stayed alive, how much was dropped, and which subsystem logged
    errors or Eco transitions.
    """
    if not samples:
        return {"samples": 0, "warnings": ["no samples"]}
    ordered = sorted(samples, key=lambda sample: sample.get("wall_t", sample.get("t", 0.0)))
    start = float(ordered[0].get("wall_t", ordered[0].get("t", 0.0)))
    end = float(ordered[-1].get("wall_t", ordered[-1].get("t", 0.0)))
    duration = max(0.0, end - start)
    gaps = [
        float(b.get("wall_t", b.get("t", 0.0))) - float(a.get("wall_t", a.get("t", 0.0)))
        for a, b in zip(ordered, ordered[1:])
    ]
    gaps = [gap for gap in gaps if gap > 0]
    groups: dict[str, Any] = {}
    group_names = sorted(
        {group for sample in ordered for group in (sample.get("groups") or {})}
    )
    for group in group_names:
        rss_points = [
            (
                float(sample.get("wall_t", sample.get("t", 0.0))),
                float(sample["groups"][group]["rss_mib"]),
            )
            for sample in ordered
            if (sample.get("groups") or {}).get(group, {}).get("rss_mib") is not None
        ]
        cpu_values = [
            float(sample["groups"][group]["cpu_pct"])
            for sample in ordered
            if (sample.get("groups") or {}).get(group, {}).get("cpu_pct") is not None
        ]
        threads = [
            int(sample["groups"][group]["threads"])
            for sample in ordered
            if (sample.get("groups") or {}).get(group, {}).get("threads") is not None
        ]
        processes = [
            int(sample["groups"][group]["processes"])
            for sample in ordered
            if (sample.get("groups") or {}).get(group, {}).get("processes") is not None
        ]
        groups[group] = {
            "rss_first_mib": rss_points[0][1] if rss_points else None,
            "rss_last_mib": rss_points[-1][1] if rss_points else None,
            "rss_max_mib": max((value for _, value in rss_points), default=None),
            "rss_slope_mib_per_hour": _slope_per_hour(rss_points),
            "cpu_mean_pct": statistics.fmean(cpu_values) if cpu_values else None,
            "cpu_max_pct": max(cpu_values) if cpu_values else None,
            "threads_first": threads[0] if threads else None,
            "threads_last": threads[-1] if threads else None,
            "threads_max": max(threads) if threads else None,
            "processes_first": processes[0] if processes else None,
            "processes_last": processes[-1] if processes else None,
            "processes_max": max(processes) if processes else None,
        }
    pid_sets = [
        tuple(sorted(sample.get("mpv_pids") or [])) for sample in ordered
    ]
    pid_changes = sum(1 for a, b in zip(pid_sets, pid_sets[1:]) if a != b)
    dropped_total = 0
    previous_pids: Optional[tuple[int, ...]] = None
    previous_dropped: Optional[int] = None
    for sample in ordered:
        playback = sample.get("playback") or {}
        pids = tuple(sorted(sample.get("mpv_pids") or []))
        dropped = playback.get("dropped")
        if (
            dropped is not None
            and previous_dropped is not None
            and pids == previous_pids
            and int(dropped) >= previous_dropped
        ):
            dropped_total += int(dropped) - previous_dropped
        previous_dropped = int(dropped) if dropped is not None else None
        previous_pids = pids
    fps_values = [
        float(playback["fps"])
        for sample in ordered
        if (playback := (sample.get("playback") or {})).get("fps") is not None
    ]
    state_counts: dict[str, int] = {}
    playing_false = 0
    for sample in ordered:
        playback = sample.get("playback") or {}
        if playback.get("is_playing") is False:
            playing_false += 1
        for state in (playback.get("monitor_states") or {}).values():
            state_counts[str(state)] = state_counts.get(str(state), 0) + 1
    markers = {
        "player_stopped": "[Player] stopped",
        "player_restarted": "[Player] restarted",
        "ipc_connection_failed": "[mpv] IPC connection failed",
        "command_failed": "[mpv] command failed",
        "decoder_parked": "[Eco] parked the decoder",
        "decoder_unparked": "[Eco] decoder unparked",
    }
    log_counts = {
        name: sum(1 for line in log_lines if marker in line)
        for name, marker in markers.items()
    }
    log_counts["eco_transitions"] = sum(1 for line in log_lines if "[Eco]" in line)
    warnings: list[str] = []
    if duration < planned_duration_s * 0.9:
        warnings.append(
            f"run covered {duration:.0f}s of the planned {planned_duration_s:.0f}s"
        )
    if playing_false > max(2, len(ordered) // 20):
        warnings.append(
            f"player reported not playing in {playing_false}/{len(ordered)} samples"
        )
    if log_counts["ipc_connection_failed"]:
        warnings.append("the mpv IPC connection failed during the run")
    return {
        "samples": len(ordered),
        "planned_duration_s": planned_duration_s,
        "duration_s": duration,
        "sample_gap_max_s": max(gaps) if gaps else None,
        "groups": groups,
        "mpv_pid_changes": pid_changes,
        "mpv_pids": sorted({pid for pids in pid_sets for pid in pids}),
        "dropped_total": dropped_total,
        "fps_min": min(fps_values) if fps_values else None,
        "fps_median": statistics.median(fps_values) if fps_values else None,
        "playing_false_samples": playing_false,
        "monitor_state_samples": state_counts,
        "log": log_counts,
        "warnings": warnings,
    }


def print_summary(summary: dict[str, Any], environment: dict[str, Any]) -> None:
    print(f"platform={environment.get('platform')} "
          f"version={environment.get('binary_version')} "
          f"commit={environment.get('git_commit')}")
    displays = environment.get("displays") or []
    print(f"displays={len(displays)}")
    for phase, data in summary.items():
        print(f"[{phase}] samples={data['samples']} duration={data['duration_s']:.1f}s")
        for group, metrics in (data.get("groups") or {}).items():
            def show(key: str) -> str:
                value = metrics.get(key, {}).get("median")
                return "n/a" if value is None else f"{value:.2f}"
            print(
                f"  {group:8s} cpu={show('cpu_pct')}%/core "
                f"cpu_machine={show('cpu_pct_machine')}% "
                f"rss={show('rss_mib')}MiB pss={show('pss_mib')}MiB "
                f"threads={show('threads')} procs={show('processes')} "
                f"gpu={show('gpu_pct')}% vram={show('vram_mib')}MiB"
                .replace("pss=n/aMiB", "pss=n/a")
                .replace("gpu=n/a%", "gpu=n/a")
                .replace("vram=n/aMiB", "vram=n/a")
            )
        fps = data.get("playback", {}).get("fps", {}).get("median")
        dropped = data.get("playback", {}).get("dropped", {}).get("median")
        gpu = data.get("playback", {}).get("gpu_global_pct", {}).get("median")
        print(
            f"  playback fps={'n/a' if fps is None else f'{fps:.2f}'} "
            f"dropped={'n/a' if dropped is None else f'{dropped:.1f}'} "
            f"gpu_global={'n/a' if gpu is None else f'{gpu:.1f}'}%"
        )


def default_output_dir() -> Path:
    return Path.cwd() / "bench-results"
