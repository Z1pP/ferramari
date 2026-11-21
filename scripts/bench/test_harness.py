"""Unit tests for the benchmark harness.

Run from the repository root:

    python -m unittest discover -s scripts/bench -p 'test_*.py'

They cover the parts that must not regress silently: missing values, the
per-group CPU/RSS aggregation, the GVariant reader used by the Linux backend,
the soak report and the Windows ABI structs. They do not need a display, a
player or a server.
"""

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bench import common  # noqa: E402


def proc(pid: int, ppid: int, name: str, cpu: float, rss: int, threads: int):
    return common.ProcInfo(
        pid=pid,
        ppid=ppid,
        name=name,
        cpu_seconds=cpu,
        rss_bytes=rss,
        pss_bytes=rss // 2,
        threads=threads,
    )


class GroupMetricsTests(unittest.TestCase):
    def test_new_pid_makes_cpu_partial_but_rss_is_still_measured(self):
        previous = {1: proc(1, 0, "ferramari", 1.0, 100 * common.MIB, 2)}
        current = {
            1: proc(1, 0, "ferramari", 2.0, 200 * common.MIB, 3),
            2: proc(2, 1, "mpv", 0.5, 50 * common.MIB, 1),
        }
        metrics = common.group_metrics(
            previous, current, 1.0, {"tree": {1, 2}}, None, 1
        )["tree"]
        self.assertEqual(metrics["cpu_pct"], 100.0)
        self.assertTrue(metrics["cpu_partial"])
        self.assertEqual(metrics["processes"], 2)
        self.assertAlmostEqual(metrics["rss_mib"], 250.0)
        self.assertAlmostEqual(metrics["pss_mib"], 125.0)
        self.assertEqual(metrics["threads"], 4)

    def test_stable_tree_reports_a_cpu_delta_for_every_pid(self):
        previous = {
            1: proc(1, 0, "ferramari", 1.0, 100 * common.MIB, 2),
            2: proc(2, 1, "mpv", 2.0, 50 * common.MIB, 3),
        }
        current = {
            1: proc(1, 0, "ferramari", 1.5, 100 * common.MIB, 2),
            2: proc(2, 1, "mpv", 2.5, 50 * common.MIB, 3),
        }
        metrics = common.group_metrics(
            previous, current, 2.0, {"tree": {1, 2}}, None, 1
        )["tree"]
        self.assertFalse(metrics["cpu_partial"])
        self.assertAlmostEqual(metrics["cpu_pct"], 50.0)
        self.assertAlmostEqual(metrics["cpu_pct_machine"], 50.0)

    def test_missing_gpu_and_pss_are_none_not_zero(self):
        current = {1: common.ProcInfo(1, 0, "ferramari", 0.0, None, None, None)}
        metrics = common.group_metrics({}, current, 1.0, {"tree": {1}}, None, 1)["tree"]
        self.assertIsNone(metrics["rss_mib"])
        self.assertIsNone(metrics["pss_mib"])
        self.assertIsNone(metrics["threads"])
        self.assertIsNone(metrics["gpu_pct"])
        self.assertIsNone(metrics["vram_mib"])


class SummaryTests(unittest.TestCase):
    def test_all_missing_is_zero_samples_not_zero_median(self):
        summary = common.summarize([None, None])
        self.assertEqual(summary["samples"], 0)
        self.assertIsNone(summary["median"])

    def test_missing_values_are_skipped(self):
        summary = common.summarize([1.0, None, 3.0])
        self.assertEqual(summary["samples"], 2)
        self.assertEqual(summary["median"], 2.0)

    def test_phase_duration_uses_wall_clock_for_chunked_long_runs(self):
        samples = [
            {
                "phase": "long-run",
                "sample": 0,
                "t": 0.0,
                "wall_t": 100.0,
                "groups": {},
                "playback": {},
            },
            {
                "phase": "long-run",
                "sample": 1,
                "t": 1.0,
                "wall_t": 105.0,
                "groups": {},
                "playback": {},
            },
        ]
        summary = common.summarize_samples(samples)["long-run"]
        self.assertEqual(summary["duration_s"], 5.0)

    def test_first_frame_uses_the_cold_start_flag(self):
        self.assertTrue(
            common.first_frame_reached(
                {"is_playing": True, "is_decoding": False}
            )
        )
        self.assertFalse(
            common.first_frame_reached(
                {"is_playing": True, "is_decoding": True}
            )
        )
        self.assertFalse(
            common.first_frame_reached(
                {"is_playing": False, "is_decoding": False}
            )
        )


class LinuxParsingTests(unittest.TestCase):
    def test_gvariant_booleans_and_string_maps(self):
        from bench.linux import LinuxControl

        self.assertIs(LinuxControl._value("(<true>,)"), True)
        self.assertIs(LinuxControl._value("(<false>,)"), False)
        self.assertIs(LinuxControl._value("(true,)"), True)
        self.assertEqual(
            LinuxControl._value("({'DISPLAY1': 'idle'},)"),
            {"DISPLAY1": "idle"},
        )
        self.assertEqual(
            LinuxControl._value("(<{'DISPLAY1': 'idle'}>,)"),
            {"DISPLAY1": "idle"},
        )
        self.assertIsNone(LinuxControl._value("(uint32 120,)"))

    def test_stat_parser_uses_rss_pages_and_thread_count(self):
        from bench.linux import CLK_TCK, LinuxProcs

        fields = ["S", "42"] + ["0"] * 9
        fields += ["100", "50"] + ["0"] * 4
        fields += ["7"] + ["0", "0", "0"] + ["123"]
        line = "123 (a) b c) " + " ".join(fields)
        parsed = LinuxProcs._parse_stat(line)
        self.assertIsNotNone(parsed)
        ppid, threads, rss_pages, cpu, comm = parsed
        self.assertEqual(ppid, 42)
        self.assertEqual(threads, 7)
        self.assertEqual(rss_pages, 123)
        self.assertAlmostEqual(cpu, 150 / CLK_TCK)
        self.assertEqual(comm, "a) b c")


class CsvTests(unittest.TestCase):
    def test_missing_values_are_empty_not_zero(self):
        sample = {
            "phase": "steady",
            "sample": 0,
            "t": 1.0,
            "groups": {
                "mpv": {
                    "cpu_pct": None,
                    "rss_mib": None,
                    "threads": None,
                    "processes": 0,
                    "pss_mib": None,
                    "gpu_pct": None,
                    "vram_mib": None,
                }
            },
            "playback": {"is_playing": True, "fps": None},
            "gpu_global_pct": None,
            "vram_global_mib": None,
            "mpv_pids": [],
            "notes": [],
        }
        row = common.sample_to_row("steady", 0, sample)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.csv"
            common.write_csv(path, [row])
            with path.open(encoding="utf-8", newline="") as handle:
                rows = list(csv.DictReader(handle))
        self.assertEqual(rows[0]["mpv_cpu_pct"], "")
        self.assertEqual(rows[0]["mpv_rss_mib"], "")
        self.assertEqual(rows[0]["gpu_global_pct"], "")


class SoakTests(unittest.TestCase):
    def _sample(self, wall_t, rss, threads, pid, dropped):
        return {
            "phase": "long-run",
            "sample": 0,
            "t": wall_t,
            "wall_t": wall_t,
            "groups": {
                "total": {
                    "cpu_pct": 5.0,
                    "rss_mib": rss,
                    "threads": threads,
                    "processes": 3,
                    "pss_mib": None,
                    "gpu_pct": 10.0,
                    "vram_mib": 20.0,
                }
            },
            "playback": {
                "is_playing": True,
                "is_decoding": False,
                "fps": 30.0,
                "dropped": dropped,
                "monitor_states": {"DISPLAY1": "active"},
            },
            "mpv_pids": [pid],
        }

    def test_slope_pid_change_and_dropped_delta(self):
        samples = [
            self._sample(0, 100.0, 10, 10, 0),
            self._sample(60, 101.0, 10, 10, 2),
            self._sample(120, 103.0, 11, 11, 5),
        ]
        summary = common.soak_summary(
            samples,
            [
                "[Player] restarted DISPLAY1",
                "[Eco] parked the decoder",
                "[mpv] command failed",
            ],
            120.0,
        )
        self.assertEqual(summary["samples"], 3)
        self.assertEqual(summary["duration_s"], 120.0)
        self.assertAlmostEqual(
            summary["groups"]["total"]["rss_slope_mib_per_hour"], 90.0
        )
        self.assertEqual(summary["mpv_pid_changes"], 1)
        self.assertEqual(summary["dropped_total"], 2)
        self.assertEqual(summary["log"]["player_restarted"], 1)
        self.assertEqual(summary["log"]["decoder_parked"], 1)
        self.assertEqual(summary["log"]["command_failed"], 1)
        self.assertEqual(summary["warnings"], [])

    def test_empty_soak_is_flagged(self):
        summary = common.soak_summary([], [], 60.0)
        self.assertEqual(summary["samples"], 0)
        self.assertTrue(summary["warnings"])


class WindowsAbiTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "win32", "Windows-only ABI check")
    def test_process_entry_layout_is_568_bytes(self):
        import ctypes

        from bench import windows

        self.assertEqual(ctypes.sizeof(windows.PROCESSENTRY32W), 568)


if __name__ == "__main__":
    unittest.main()
