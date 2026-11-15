"""Deterministic phase attribution using a private fake proc tree; no child processes."""
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from measurement import Metrics, Tree
import proc_sampler

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import summarize


class PhaseIntervalTests(unittest.TestCase):
    def process(self, proc, cpu=0, rss=100):
        path = proc/'100'
        path.mkdir(parents=True, exist_ok=True)
        fields = ['0'] * 22
        fields[0], fields[1], fields[11], fields[12], fields[19] = 'S', '1', str(cpu), '0', '100'
        (path/'stat').write_text('100 (synthetic) '+' '.join(fields))
        (path/'smaps_rollup').write_text(f'Rss: {rss} kB\nPss: 80 kB\nSwapPss: 2 kB\nPrivate_Clean: 3 kB\nPrivate_Dirty: 4 kB\n')
        (path/'io').write_text(f'read_bytes: {cpu*10}\nwrite_bytes: {cpu*20}\nrchar: {cpu*30}\nwchar: {cpu*40}\n')
        (path/'status').write_text('VmHWM: 10000 kB\nThreads: 1\n')
        (path/'cmdline').write_bytes(b'/synthetic/backend\0')
        (proc/'uptime').write_text('1000 0\n')

    def scenario(self, root, *, boundary_sample=False, finish=True, boundary_rss=100):
        proc = root/'proc'
        case = root/'case'
        case.mkdir()
        self.process(proc)
        tree = Tree(100, root_group='backend', proc=proc)
        tree.clock_ticks = 100
        clock = SimpleNamespace(ns=1_000_000_000)
        args = SimpleNamespace(max_pss_mib=1024, timeout=60, interval=0.1)
        with patch.object(proc_sampler.time, 'monotonic_ns', side_effect=lambda: clock.ns), \
             patch.object(proc_sampler.time, 'monotonic', side_effect=lambda: clock.ns/1e9):
            metrics = Metrics(tree, case, args)
            try:
                metrics.mark('first_launch')
                metrics.sample()
                clock.ns = 2_000_000_000
                self.process(proc, cpu=100, rss=boundary_rss)
                if boundary_sample:
                    metrics.sample()
                metrics.mark('resident_idle')
                if not boundary_sample:
                    metrics.sample()
                if finish:
                    clock.ns = 3_000_000_000
                    self.process(proc, cpu=100)
                    metrics.sample()
                measurement = metrics.summary()
            finally:
                metrics.close()
        events = [json.loads(line) for line in (case/'samples.jsonl').read_text().splitlines()]
        return case, measurement, events

    def render(self, root, case, measurement):
        source = root/'report.json'
        output = root/'comparison.md'
        source.write_text(json.dumps({'status': 'complete', 'mode': 'agent-control', 'runs': [
            {'status': 'passed', 'mode': 'agent-control', 'messages_per_session': 1,
             'peers': 1, 'path': str(case), 'measurement': measurement}]}))
        original = {path: path.read_bytes() for path in [source, case/'samples.jsonl']}
        with patch.object(sys, 'argv', ['summarize.py', '--active-before', str(source), '--output', str(output)]), \
             redirect_stdout(io.StringIO()):
            summarize.main()
        self.assertEqual(original, {path: path.read_bytes() for path in original})
        return output.read_text()

    def full_tree_row(self, text):
        return next(line for line in text.splitlines() if line.startswith('|') and '+ управляющий процесс' in line)

    def test_phase_summary_does_not_charge_launch_work_to_idle(self):
        with tempfile.TemporaryDirectory() as directory:
            _, measurement, events = self.scenario(Path(directory), boundary_rss=400)
            idle = measurement['phases']['resident_idle']
            # Behavioral red assertion occurs before access to new evidence fields.
            self.assertEqual(idle['cpu_percent_one_core_weighted_mean'], 0)
            self.assertEqual(idle['cpu_percent_one_core'], {'median': 0, 'peak': 0})
            self.assertEqual(idle['observed_interval_counters']['cpu_ticks'], 0)
            self.assertEqual(idle['excluded_interval_counters']['cpu_ticks'], 100)
            self.assertEqual(idle['excluded_interval_counters']['read_bytes'], 1000)
            self.assertEqual(idle['observed_interval_seconds'], 1)
            self.assertEqual(idle['excluded_interval_seconds'], 1)
            self.assertEqual(idle['eligible_interval_samples'], 1)
            self.assertEqual(idle['excluded_interval_samples'], 1)
            self.assertEqual(idle['samples'], 2)
            self.assertEqual(idle['rss_kib'], {'median': 250, 'peak': 400})
            self.assertEqual(measurement['subject_observed_counters']['cpu_ticks'], 100)
            self.assertEqual(measurement['full_tree']['cpu_ticks'], 100)
            samples = [event for event in events if event.get('type') == 'sample' and event['phase'] == 'resident_idle']
            self.assertEqual(samples[0]['interval_counters']['cpu_ticks'], 100)
            self.assertEqual(samples[0]['interval_start_monotonic_ns'], 1_000_000_000)
            self.assertFalse(samples[0]['phase_interval_eligible'])
            self.assertTrue(samples[1]['phase_interval_eligible'])

    def test_full_tree_comparison_does_not_charge_launch_work_to_idle(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case, measurement, _ = self.scenario(root)
            text = self.render(root, case, measurement)
            self.assertEqual(self.full_tree_row(text).split('|')[-2].strip(), '0.00')
            self.assertIn('1/2', text)

    def test_explicit_old_phase_boundary_sample_preserves_attribution(self):
        with tempfile.TemporaryDirectory() as directory:
            _, measurement, _ = self.scenario(Path(directory), boundary_sample=True)
            launch = measurement['phases']['first_launch']
            idle = measurement['phases']['resident_idle']
            self.assertEqual(launch['observed_interval_counters']['cpu_ticks'], 100)
            self.assertEqual(launch['cpu_percent_one_core_weighted_mean'], 100)
            self.assertEqual(idle['cpu_percent_one_core_weighted_mean'], 0)
            self.assertEqual(idle['eligible_interval_samples'], 1)
            self.assertEqual(idle['excluded_interval_samples'], 0)

    def test_every_marker_starts_an_epoch_even_when_label_returns_or_repeats(self):
        tracker = proc_sampler.PhaseIntervals()
        def mark(phase, time):
            tracker.observe({'type': 'phase', 'phase': phase, 'monotonic_ns': time})
        def sample(start, end, phase='A'):
            return tracker.observe({'type': 'sample', 'phase': phase, 'interval_start_monotonic_ns': start,
                                    'monotonic_ns': end, 'interval_seconds': (end-start)/1e9})
        mark('A', 0)
        self.assertTrue(sample(0, 10))
        mark('B', 12)
        mark('A', 15)
        self.assertFalse(sample(10, 20))
        self.assertTrue(sample(20, 30))
        mark('A', 35)
        self.assertFalse(sample(30, 40))
        self.assertTrue(sample(40, 50))

    def test_unknown_or_empty_interval_coverage_is_not_zero_cpu(self):
        with tempfile.TemporaryDirectory() as directory:
            _, measurement, _ = self.scenario(Path(directory), finish=False)
            idle = measurement['phases']['resident_idle']
            self.assertIsNone(idle['cpu_percent_one_core_weighted_mean'])
            self.assertEqual(idle['cpu_percent_one_core'], {'median': None, 'peak': None})
            self.assertEqual(idle['rss_kib']['median'], 100)
            self.assertEqual(idle['excluded_interval_counters']['cpu_ticks'], 100)
        tracker = proc_sampler.PhaseIntervals()
        sample = {'type': 'sample', 'phase': 'idle', 'interval_start_monotonic_ns': 10,
                  'monotonic_ns': 20, 'interval_seconds': 1e-8}
        self.assertFalse(tracker.observe(sample))
        tracker.observe({'type': 'phase', 'phase': 'idle', 'monotonic_ns': 10})
        for changes in [{'interval_start_monotonic_ns': None}, {'interval_start_monotonic_ns': True},
                        {'interval_seconds': None}, {'interval_seconds': 0}, {'interval_seconds': -1},
                        {'interval_seconds': float('nan')}, {'monotonic_ns': 10}, {'phase': 'different'}]:
            with self.subTest(changes=changes):
                self.assertFalse(tracker.observe({**sample, **changes}))

    def test_legacy_raw_samples_keep_memory_but_not_unproven_full_tree_cpu(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case, measurement, events = self.scenario(root)
            for event in events:
                event.pop('interval_start_monotonic_ns', None)
                event.pop('phase_interval_eligible', None)
            (case/'samples.jsonl').write_text(''.join(json.dumps(event)+'\n' for event in events))
            measurement.pop('phase_interval_policy')
            measurement['phases']['resident_idle']['cpu_percent_one_core_weighted_mean'] = 50
            text = self.render(root, case, measurement)
            full = self.full_tree_row(text)
            self.assertEqual(full.split('|')[-2].strip(), 'н/д')
            self.assertIn('0.10', full)
            subject = next(line for line in text.splitlines() if line.startswith('|')
                           and ': resident_idle ' in line and '+ управляющий процесс' not in line)
            self.assertEqual(subject.split('|')[-2].strip(), '50.00')
            self.assertIn('без ретроактивного пересчёта', text)

    def test_same_phase_weighted_cpu_and_all_memory_samples_are_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.process(root/'proc')
            tree = Tree(100, root_group='backend', proc=root/'proc')
            tree.clock_ticks = 100
            clock = SimpleNamespace(ns=1_000_000_000)
            args = SimpleNamespace(max_pss_mib=1024, timeout=60)
            with patch.object(proc_sampler.time, 'monotonic_ns', side_effect=lambda: clock.ns), \
                 patch.object(proc_sampler.time, 'monotonic', side_effect=lambda: clock.ns/1e9):
                metrics = Metrics(tree, root, args)
                try:
                    metrics.mark('idle')
                    metrics.sample()
                    clock.ns = 1_100_000_000
                    self.process(root/'proc', cpu=10, rss=200)
                    metrics.sample()
                    clock.ns = 2_000_000_000
                    self.process(root/'proc', cpu=10, rss=300)
                    metrics.sample()
                    idle = metrics.summary()['phases']['idle']
                finally:
                    metrics.close()
            self.assertAlmostEqual(idle['cpu_percent_one_core_weighted_mean'], 10)
            self.assertEqual(idle['cpu_percent_one_core'], {'median': 50, 'peak': 100})
            self.assertEqual(idle['rss_kib'], {'median': 200, 'peak': 300})
            self.assertEqual(idle['eligible_interval_samples'], 2)
            self.assertEqual(idle['excluded_interval_samples'], 1)


if __name__ == '__main__':
    unittest.main()
