#!/usr/bin/env python3
"""Compare existing performance reports; no builds or measured processes are started."""
import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import statistics


def get(value, *keys):
    for key in keys:
        value = value.get(key) if isinstance(value, dict) else None
    return value


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def cell(values, divisor=1):
    values = [value / divisor for value in values if number(value)]
    if not values:
        return 'н/д'
    result = f'{statistics.median(values):.2f}'
    if len(values) > 1:
        result += f' [{min(values):.2f}–{max(values):.2f}]'
    return result


def escape(value):
    return str(value).replace('|', '\\|').replace('\n', ' ')


def table(headers, rows):
    return ['| ' + ' | '.join(map(escape, headers)) + ' |',
            '| ' + ' | '.join('---' for _ in headers) + ' |'] + [
                '| ' + ' | '.join(map(escape, row)) + ' |' for row in rows] + ['']


def profile(report):
    command = report.get('command', [])
    return command[command.index('--profile') + 1] if '--profile' in command else report.get('profile')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for kind in ('native', 'idle', 'active', 'build'):
        for side in ('before', 'after'):
            parser.add_argument(f'--{kind}-{side}', type=Path,
                                action='append' if kind in ('idle', 'active') else 'store')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error(f'output already exists: {args.output}')
    memory, readiness, builds = defaultdict(lambda: defaultdict(list)), defaultdict(lambda: defaultdict(list)), {}
    failed, notices, sources = [], [], []
    loaded = {}
    for kind in ('native', 'idle', 'active', 'build'):
        for side in ('before', 'after'):
            paths = getattr(args, f'{kind}_{side}') or []
            if isinstance(paths, Path):
                paths = [paths]
            for path in dict.fromkeys(path.resolve() for path in paths):
                report = json.loads(path.read_text())
                sources.append((kind, side, path))
                loaded[(kind, side, path)] = report
    if not loaded:
        parser.error('provide at least one input report')
    build_profiles = {profile(report) for (kind, _, _), report in loaded.items() if kind == 'build'}
    if len(build_profiles) > 1:
        parser.error(f'build profiles differ: {build_profiles}; compare fast/release separately')
    for (kind, side, path), report in loaded.items():
        label = 'до' if side == 'before' else 'после'
        if report.get('status') == 'running':
            notices.append(f'{path}: прогон ещё выполняется; учтены только завершённые успешные кейсы.')
        if kind == 'build':
            if report.get('status') != 'complete':
                failed.append(f'Сборка {label}, {path}: {report.get("error", report.get("status", "нет status"))}')
            else:
                builds[side] = report
            continue
        rows = report.get('results' if kind == 'native' else 'runs', [report])
        if not rows:
            notices.append(f'{path}: завершённых кейсов нет.')
        for row in rows:
            messages = row.get('messages' if kind == 'native' else 'messages_per_session', 'н/д')
            peers = 1 if kind == 'native' else row.get('peers', 'н/д')
            scope = {'native': 'GUI', 'idle': 'HTTP backends', 'active': 'AgentControl'}[kind]
            scenario = f'{scope}: {messages} сообщений × {peers} сессий'
            model_rows = row.get('model_history_rows', report.get('model_history_rows'))
            if kind == 'active':
                scenario += f': model history {model_rows if model_rows is not None else messages}'
            measurement = row if kind == 'native' else row.get('measurement', {})
            wrong_mode = kind != 'native' and row.get('mode', report.get('mode')) != (
                'resume-idle' if kind == 'idle' else 'agent-control')
            if row.get('status') != 'passed' or measurement.get('status') == 'failed' or wrong_mode:
                reason = ('режим отчёта не соответствует аргументу CLI' if wrong_mode else
                          row.get('error') or measurement.get('error') or row.get('status', 'нет status'))
                failed.append(f'{scenario}, {label}, repeat={row.get("repeat", "н/д")}, {path}: {reason}')
                continue
            if kind == 'native':
                scenario += ': ' + row.get('cache', 'н/д')
                peak = row.get('measured_peak_kib') or get(row, 'counters', 'sampled_simultaneous_peaks_kib') or {}
                memory[scenario][side].append(dict(**row.get('median', {}), peak_rss=peak.get('rss_kib'),
                    peak_pss_swap=peak.get('pss_swap_kib'), cpu=row.get('idle_cpu_percent_one_core')))
                for stage in ('backend.ready', 'window.create', 'chat.snapshot.applied', 'chat.transcript.render'):
                    readiness[(scenario, stage + ', мс')][side].append(get(row, 'stages', stage, 'elapsed_ms'))
                external = row.get('external_readiness_observed_s')
                readiness[(scenario, 'внешнее наблюдение готовности, мс')][side].append(external * 1000 if number(external) else None)
            else:
                phases = measurement.get('phases', {})
                memory[scenario + ': пик всего кейса'][side].append(dict(
                    peak_rss=measurement.get('sampled_peak_rss_kib'),
                    peak_pss_swap=measurement.get('sampled_peak_pss_swap_kib')))
                wanted = ('first_resident_idle', 'restart_resident_idle') if kind == 'idle' else tuple(
                    phase for phase in phases if phase not in ('launch', 'teardown'))
                for phase in wanted:
                    data = phases.get(phase, {})
                    if not data:
                        continue
                    values = {key: get(data, key, 'median') for key in ('rss_kib', 'pss_kib', 'swap_pss_kib', 'pss_swap_kib')}
                    values.update(peak_rss=get(data, 'rss_kib', 'peak'), peak_pss_swap=get(data, 'pss_swap_kib', 'peak'),
                                  cpu=data.get('cpu_percent_one_core_weighted_mean'))
                    memory[scenario + ': ' + phase][side].append(values)
                if kind == 'idle':
                    for cache in ('first', 'restart'):
                        for key in ('all_http_bound_ms', 'all_snapshots_ready_ms'):
                            readiness[(scenario + ': ' + cache, key + ', мс')][side].append(get(measurement, 'readiness', cache, key))
                else:
                    events = measurement.get('events', [])
                    samples_path = Path(row.get('path', path.parent)) / 'samples.jsonl'
                    if samples_path.is_file():
                        samples = [item for line in samples_path.read_text().splitlines()
                                   if (item := json.loads(line)).get('type') == 'sample'
                                   and item.get('phase') == 'resident_idle']
                        if samples:
                            values = {key: statistics.median(item[key] for item in samples)
                                      for key in ('rss_kib','pss_kib','swap_pss_kib','pss_swap_kib')}
                            intervals = [item for item in samples if item.get('interval_seconds')]
                            values.update(peak_rss=max(item['rss_kib'] for item in samples),
                                          peak_pss_swap=max(item['pss_swap_kib'] for item in samples),
                                          cpu=sum((item.get('interval_cpu_percent_one_core') or 0)*item['interval_seconds'] for item in intervals)
                                          / sum(item['interval_seconds'] for item in intervals) if intervals else None)
                            memory[scenario+': resident_idle + управляющий процесс'][side].append(values)
                    for phase in ('parent_ready', 'all_models_entered', 'round_settled'):
                        elapsed = [event.get('elapsed_ms') for event in events if event.get('phase') == phase]
                        readiness[(scenario, phase + ' (первое событие), мс')][side].append(elapsed[0] if elapsed else None)
                    readiness[(scenario, 'максимум параллельных model calls')][side].append(get(measurement, 'model_overlap', 'maximum_in_flight'))
    lines = ['# Сравнение производительности Proteus', '',
             'В ячейках: медиана независимых повторов [минимум–максимум]; n — число успешных кейсов. '
             'Память — MiB (KiB / 1024). Отсутствующие значения: н/д. Проваленные кейсы исключены.', '',
             'GUI включает native, backend и дочерние modules/extensions. HTTP backends — независимые процессы Core '
             'и modules без GUI; это сценарий resume-idle. AgentControl — настоящие spawn/wait/resume с fake model '
             'и read_file; Python/driver overhead исключён из измеряемой памяти. Число сообщений задано в каждой сессии.', '',
             'Дополнительные строки «+ управляющий процесс» включают Rust AgentControl driver и всё его дерево; внешний Python измеритель в них не входит.', '',
             'CPU — среднее с весом по времени внутри фазы (100% = одно ядро), затем медиана повторов. '
             'RSS суммирует разделяемые страницы; PSS распределяет их. PSS+SwapPss учитывает вытеснение в swap. '
             'Пики — максимумы одновременных сэмплов внутри каждого кейса/фазы, затем медиана повторов; '
             'короткие пики между сэмплами могут быть пропущены. OS cache не сбрасывается; first — пустой кэш приложения, restart — повторный процесс.', '']
    if notices:
        lines += ['## Незавершённые прогоны', ''] + [f'- {escape(item)}' for item in notices] + ['']
    lines += ['## Память и CPU', '']
    memory_rows = []
    for scenario, sides in sorted(memory.items()):
        for side in ('before', 'after'):
            rows = sides.get(side, [])
            memory_rows.append([scenario, 'до' if side == 'before' else 'после', len(rows)] + [
                cell([row.get(key) for row in rows], divisor) for key, divisor in (
                    ('rss_kib', 1024), ('pss_kib', 1024), ('swap_pss_kib', 1024), ('pss_swap_kib', 1024),
                    ('peak_rss', 1024), ('peak_pss_swap', 1024), ('cpu', 1))])
    lines += table(['Сценарий', 'Версия', 'n', 'RSS MiB', 'PSS MiB', 'SwapPss MiB', 'PSS+Swap MiB',
                    'Пик RSS MiB', 'Пик PSS+Swap MiB', 'CPU %'], memory_rows)
    lines += ['## Готовность', '', 'Внутренние GUI milestones отсчитываются от запуска native. '
              'window.create подтверждает создание окна; chat.transcript.render — выполнение рендера истории. '
              'Внешнее наблюдение включает задержку polling. HTTP ready означает доступность всех запущенных peers.', '']
    lines += table(['Сценарий', 'Показатель', 'До', 'После'], [
        [scenario, metric, cell(sides.get('before', [])), cell(sides.get('after', []))]
        for (scenario, metric), sides in sorted(readiness.items())])
    if builds:
        lines += ['## Сборка', '', f'Профиль: {next(iter(build_profiles)) or "н/д"}. '
                  'warmup и restore представлены отдельно от стоимости edit; target сохраняется. '
                  'rebuilt_artifacts — число Cargo targets, не число уникальных packages.', '']
        build_rows = []
        for phase in ('warmup', 'noop', 'edit', 'restore'):
            for side in ('before', 'after'):
                report = builds.get(side, {})
                rows = [row for row in report.get('phases', []) if row.get('exit_code') == 0
                        and (row.get('phase') == phase or row.get('phase', '').endswith('-' + phase))]
                build_rows.append([phase, 'до' if side == 'before' else 'после', len(rows)] + [
                    cell([row.get(key) for row in rows]) for key in ('wall_seconds', 'user_seconds', 'system_seconds', 'rebuilt_artifacts')])
        lines += table(['Фаза', 'Версия', 'n', 'Wall с', 'User с', 'System с', 'Rebuilt artifacts'], build_rows)
    lines += ['## Исключённые кейсы', ''] + ([f'- {escape(item)}' for item in failed] if failed else ['Нет в переданных отчётах.']) + ['']
    lines += ['## Исходные отчёты', ''] + [f'- {kind}, {side}: [{escape(path)}](<{path}>)' for kind, side, path in sources]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as stream:
        stream.write('\n'.join(lines) + '\n')
    print(args.output)


if __name__ == '__main__':
    main()
