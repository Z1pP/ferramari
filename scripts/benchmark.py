#!/usr/bin/env python3
"""Measure Ferramari's process tree on a real desktop.

The script starts an isolated instance on a private D-Bus session, applies a
local video, samples /proc for a few seconds, optionally kills mpv and measures
recovery, then quits. It does not touch the user's config, cache or D-Bus name.

Requires: Python 3, gdbus, dbus-daemon, a running X11/XWayland display, mpv.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

DBUS_NAME = "io.github.ferramari.Ferramari.server"
OBJECT = "/io/github/ferramari/ferramari/server"
INTERFACE = "io.github.ferramari.ferramari.server"
CLK_TCK = os.sysconf("SC_CLK_TCK")


def dbus_env(address: str) -> dict[str, str]:
    env = os.environ.copy()
    env["DBUS_SESSION_BUS_ADDRESS"] = address
    return env


def gdbus(address: str, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["gdbus", *args],
        env=dbus_env(address),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=10,
    )


def wait_for_name(address: str, deadline: float) -> bool:
    while time.time() < deadline:
        result = gdbus(
            address,
            "call",
            "--session",
            "--dest",
            DBUS_NAME,
            "--object-path",
            OBJECT,
            "--method",
            "org.freedesktop.DBus.Peer.Ping",
        )
        if result.returncode == 0:
            return True
        time.sleep(0.05)
    return False


def call(address: str, method: str, *args: str) -> subprocess.CompletedProcess[str]:
    return gdbus(
        address,
        "call",
        "--session",
        "--dest",
        DBUS_NAME,
        "--object-path",
        OBJECT,
        "--method",
        method,
        *args,
    )


def read_proc(pid: int) -> tuple[int, int, int, str, int] | None:
    """Return (ppid, ticks, rss_kb, comm, threads) for one pid."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        status = Path(f"/proc/{pid}/status").read_text()
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return None
    close = stat.rfind(")")
    if close < 0:
        return None
    fields = stat[close + 1 :].split()
    if len(fields) < 13:
        return None
    ppid = int(fields[1])
    ticks = int(fields[11]) + int(fields[12])
    comm = stat[stat.find("(") + 1 : close]
    rss_kb = 0
    threads = 1
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            rss_kb = int(line.split()[1])
        elif line.startswith("Threads:"):
            threads = int(line.split()[1])
    return ppid, ticks, rss_kb, comm, threads


def process_tree(root: int) -> dict:
    all_pids: dict[int, tuple[int, int, int, str, int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        info = read_proc(int(entry.name))
        if info:
            all_pids[int(entry.name)] = info
    children: dict[int, list[int]] = {}
    for pid, (ppid, *_rest) in all_pids.items():
        children.setdefault(ppid, []).append(pid)
    stack = [root]
    tree = {"pids": [], "ticks": 0, "rss_kb": 0, "threads": 0, "mpv": []}
    while stack:
        pid = stack.pop()
        info = all_pids.get(pid)
        if not info:
            continue
        _ppid, ticks, rss_kb, comm, threads = info
        tree["pids"].append(pid)
        tree["ticks"] += ticks
        tree["rss_kb"] += rss_kb
        tree["threads"] += threads
        if comm == "mpv":
            tree["mpv"].append(pid)
        stack.extend(children.get(pid, []))
    tree["processes"] = len(tree["pids"])
    return tree


def gpu_percent() -> float | None:
    values = []
    for path in Path("/sys/class/drm").glob("card*/device/gpu_busy_percent"):
        try:
            values.append(float(path.read_text().strip()))
        except (OSError, ValueError):
            pass
    return max(values) if values else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        default="target/release/ferramari",
        help="Ferramari binary (default: target/release/ferramari)",
    )
    parser.add_argument("--video", required=True, help="local video to play")
    parser.add_argument(
        "--profile",
        default="balanced",
        choices=["performance", "balanced", "power"],
        help="playback profile written to the isolated config",
    )
    parser.add_argument("--seconds", type=float, default=10.0)
    parser.add_argument("--csv", help="optional CSV output path")
    parser.add_argument(
        "--recover",
        action="store_true",
        help="SIGKILL mpv once and measure how long until a new mpv appears",
    )
    args = parser.parse_args()

    binary = Path(args.binary).resolve()
    video = Path(args.video).resolve()
    if not binary.is_file():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2
    if not video.is_file():
        print(f"video not found: {video}", file=sys.stderr)
        return 2
    if not os.environ.get("DISPLAY"):
        print("DISPLAY is not set; video playback needs X11/XWayland", file=sys.stderr)
        return 2

    root = Path(tempfile.mkdtemp(prefix="ferramari-bench-"))
    config = root / "config"
    cache = root / "cache"
    (config / "ferramari").mkdir(parents=True)
    cache.mkdir()
    (config / "user-dirs.dirs").write_text(f'XDG_VIDEOS_DIR="{root / "videos"}"\n')
    (root / "videos").mkdir()
    (config / "ferramari" / "config.json").write_text(
        json.dumps(
            {
                "version": 4,
                "mode": "MODE_NULL",
                "data_source": {"Default": ""},
                "playback_profile": args.profile,
                "is_first_time": False,
                "is_static_wallpaper": False,
                "idle_timeout_sec": 300,
            }
        )
        + "\n"
    )

    bus = subprocess.run(
        ["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    ).stdout.splitlines()
    address, bus_pid = bus[0], int(bus[1])

    env = os.environ.copy()
    env.update(
        {
            "DBUS_SESSION_BUS_ADDRESS": address,
            "XDG_CONFIG_HOME": str(config),
            "XDG_CACHE_HOME": str(cache),
            "GSETTINGS_BACKEND": "memory",
        }
    )
    started = time.time()
    app = subprocess.Popen(
        [str(binary), "-b", "-d"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    rows = []
    try:
        if not wait_for_name(address, time.time() + 15):
            print("Ferramari did not claim its D-Bus name", file=sys.stderr)
            return 3
        startup_ms = (time.time() - started) * 1000
        result = call(
            address,
            f"{INTERFACE}.video",
            str(video),
            "all",
        )
        if result.returncode != 0:
            print(f"could not apply video: {result.stderr}", file=sys.stderr)
            return 4
        time.sleep(2.5)

        previous = process_tree(app.pid)
        previous_at = time.time()
        killed_at = None
        killed_pid = None
        recovered_ms = None
        sample_count = max(1, int(args.seconds))
        for _ in range(sample_count):
            time.sleep(1.0)
            tree = process_tree(app.pid)
            now = time.time()
            elapsed = now - previous_at
            ticks = max(0, tree["ticks"] - previous["ticks"])
            cpu = ticks / CLK_TCK / elapsed * 100 if elapsed > 0 else 0.0
            rows.append(
                {
                    "cpu_percent": round(cpu, 2),
                    "rss_mib": round(tree["rss_kb"] / 1024.0, 1),
                    "threads": tree["threads"],
                    "processes": tree["processes"],
                    "gpu_percent": gpu_percent(),
                    "mpv": len(tree["mpv"]),
                }
            )
            if args.recover and killed_at is None and tree["mpv"]:
                killed_pid = tree["mpv"][0]
                os.kill(killed_pid, signal.SIGKILL)
                killed_at = now
            if killed_at is not None and recovered_ms is None:
                if any(pid != killed_pid for pid in tree["mpv"]):
                    recovered_ms = (now - killed_at) * 1000
            previous, previous_at = tree, now

        print(f"startup_ms={startup_ms:.0f}")
        if recovered_ms is not None:
            print(f"mpv_recovery_ms={recovered_ms:.0f}")
        print("sample,cpu_percent,rss_mib,threads,processes,gpu_percent,mpv")
        for index, row in enumerate(rows, 1):
            print(
                f"{index},{row['cpu_percent']},{row['rss_mib']},{row['threads']},"
                f"{row['processes']},{'' if row['gpu_percent'] is None else row['gpu_percent']},"
                f"{row['mpv']}"
            )
        if args.csv:
            with open(args.csv, "w", encoding="utf-8") as handle:
                handle.write("sample,cpu_percent,rss_mib,threads,processes,gpu_percent,mpv\n")
                for index, row in enumerate(rows, 1):
                    gpu = "" if row["gpu_percent"] is None else row["gpu_percent"]
                    handle.write(
                        f"{index},{row['cpu_percent']},{row['rss_mib']},"
                        f"{row['threads']},{row['processes']},{gpu},{row['mpv']}\n"
                    )
        if rows:
            cpu = sum(row["cpu_percent"] for row in rows) / len(rows)
            rss = max(row["rss_mib"] for row in rows)
            print(f"average_cpu_percent={cpu:.2f}")
            print(f"peak_rss_mib={rss:.1f}")
        return 0
    finally:
        try:
            call(address, f"{INTERFACE}.quit")
        except Exception:
            pass
        time.sleep(1)
        try:
            os.killpg(app.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(app.pid, signal.SIGKILL)
        try:
            os.kill(bus_pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())