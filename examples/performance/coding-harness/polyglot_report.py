#!/usr/bin/env python3
"""Summarize paired Polyglot runs without treating missing trials as failures.

Final cumulative metrics are counted once; per-turn cumulative snapshots are
never summed. Missing usage remains unknown, with observed lower bounds kept
separately. Success requires both the independent grader and a clean pipeline.
Confidence intervals resample exercise slugs as paired clusters, retaining all
language variants and both harnesses together. Partial-run intervals describe
only the completed paired cohort, not the entire benchmark or unseen tasks.
Manifest exclusions are fixed by upstream controls before model outcomes;
non-evaluable cases are corpus coverage, never missing jobs or model failures.
"""
import argparse
from collections import Counter, defaultdict
import csv
import json
from pathlib import Path
import random
from statistics import median

TOKENS = ('input_tokens', 'cached_input_tokens', 'cache_creation_input_tokens',
          'output_tokens', 'reasoning_output_tokens')
SEED = 20261116


def number(value):
    return type(value) in (int, float)


def totals(values):
    known = [value for value in values if number(value)]
    return {'total': sum(known) if values and len(known) == len(values) else None,
            'observed_lower_bound': sum(known) if known else None,
            'known_trials': len(known), 'unknown_trials': len(values) - len(known)}


def timings(values):
    known = [value for value in values if number(value)]
    return {**totals(values),
            'mean_seconds_observed': sum(known)/len(known) if known else None,
            'median_seconds_observed': median(known) if known else None}


def rate(successes, count):
    return successes / count if count else None


def infra(result):
    return (result.get('infrastructure_failure') is True
            or result.get('failure_kind') == 'infrastructure'
            or str(result.get('failure_kind', '')).startswith('infrastructure_'))


def project(case, agent, path):
    row = {'agent': agent, 'case': case['id'], 'language': case['language'],
           'exercise': case['exercise'], 'result_path': str(path), 'status': 'unrun',
           'evaluable': case.get('evaluable', True) is True,
           'evaluation_reason': case.get('evaluation_reason'),
           'baseline_pass': case.get('baseline_pass')}
    if not row['evaluable']:
        row['status'] = 'excluded'
        return row
    if not path.is_file():
        return row
    result = json.loads(path.read_text())
    if result.get('agent', agent) != agent or result.get('case', case['id']) != case['id']:
        raise ValueError(f'Trial identity differs from the frozen manifest: {path}')
    metrics = result.get('metrics') or {}
    usage = metrics.get('provider_usage') or {}
    partial = metrics.get('partial_reported_usage_sums') or {}
    complete = result.get('usage_complete') is True
    row.update(status='recorded', pipeline_ok=result.get('pipeline_ok') is True,
               first_pass=result.get('first_pass') is True and result.get('pipeline_ok') is True,
               passed=result.get('passed') is True and result.get('pipeline_ok') is True,
               grader_pass=bool(result.get('grades')) and result['grades'][-1].get('passed') is True,
               infrastructure_failure=infra(result), failure_kind=result.get('failure_kind'),
               usage_complete=complete, usage_cost_usd=result.get('usage_cost_usd'),
               observed_cost_usd_lower_bound=result.get('observed_cost_usd_lower_bound'),
               wall_seconds=result.get('wall_seconds'), trial_wall_seconds=result.get('trial_wall_seconds'),
               model_requests=metrics.get('model_requests'),
               primary_model_requests=metrics.get('primary_model_requests'),
               all_tool_calls=metrics.get('all_tool_calls'),
               usage_includes_auxiliary_requests=metrics.get('usage_includes_auxiliary_requests'),
               auth_removed=result.get('auth_removed'),
               turns=len(result.get('turns') or []))
    row['tokens'] = {key: usage.get(key) if complete and type(usage.get(key)) is int else None
                     for key in TOKENS}
    row['observed_tokens'] = {key: usage.get(key) if type(usage.get(key)) is int else partial.get(key)
                              for key in TOKENS}
    return row


def summarize(rows):
    corpus_planned = len(rows)
    rows = [row for row in rows if row.get('evaluable', True)]
    attempted = [row for row in rows if row['status'] == 'recorded']
    n = len(attempted)
    passed = sum(row['passed'] for row in attempted)
    first = sum(row['first_pass'] for row in attempted)
    token_totals = {key: totals([row['tokens'][key] for row in attempted]) for key in TOKENS}
    for key in TOKENS:
        observed = [row['observed_tokens'][key] for row in attempted
                    if type(row['observed_tokens'][key]) is int]
        token_totals[key]['observed_lower_bound'] = sum(observed) if observed else None
    cost = totals([row['usage_cost_usd'] if row['usage_complete'] else None for row in attempted])
    observed_costs = [row['observed_cost_usd_lower_bound'] for row in attempted
                      if number(row['observed_cost_usd_lower_bound'])]
    cost['observed_lower_bound'] = sum(observed_costs) if observed_costs else None
    cache_rows = [row for row in attempted if type(row['tokens']['input_tokens']) is int
                  and type(row['tokens']['cached_input_tokens']) is int]
    cache_input = sum(row['tokens']['input_tokens'] for row in cache_rows)
    cache_read = sum(row['tokens']['cached_input_tokens'] for row in cache_rows)
    return {'planned': len(rows), 'corpus_planned': corpus_planned,
            'excluded': corpus_planned-len(rows),
            'baseline_pass_cases': sum(row.get('baseline_pass') is True for row in rows),
            'attempted': n, 'unrun': len(rows)-n,
            'complete': bool(rows) and len(rows) == n,
            'first_passes': first, 'clean_successes': passed,
            'grader_passes': sum(row['grader_pass'] for row in attempted),
            'first_pass_rate_observed': rate(first, n), 'final_rate_observed': rate(passed, n),
            'full_benchmark_score': rate(passed, n) if rows and len(rows) == n else None,
            'infrastructure_failures': sum(row['infrastructure_failure'] for row in attempted),
            'failure_kinds': dict(Counter(row['failure_kind'] or 'unclassified'
                                          for row in attempted if not row['passed'])),
            'usage_complete_trials': sum(row['usage_complete'] for row in attempted),
            'credential_cleanup_failures': sum(row['auth_removed'] is False for row in attempted),
            'credential_cleanup_unknown': sum(type(row['auth_removed']) is not bool for row in attempted),
            'tokens': token_totals, 'cost_usd': cost,
            'cost_per_clean_success_usd': cost['total']/passed if number(cost['total']) and passed else None,
            'observed_cost_per_clean_success_lower_bound_usd': (
                cost['observed_lower_bound']/passed
                if number(cost['observed_lower_bound']) and passed else None),
            'cache_hit': {'ratio': cache_read/cache_input if cache_input else None,
                          'known_trials': len(cache_rows), 'unknown_trials': n-len(cache_rows),
                          'covers_all_attempted': bool(n) and len(cache_rows) == n},
            **{key: totals([row[key] for row in attempted]) for key in
               ('model_requests', 'primary_model_requests', 'all_tool_calls')},
            **{key: timings([row[key] for row in attempted]) for key in ('wall_seconds', 'trial_wall_seconds')}}


def percentile(values, fraction):
    values = sorted(values)
    position = (len(values)-1)*fraction
    lower = int(position)
    upper = min(lower+1, len(values)-1)
    return values[lower] + (values[upper]-values[lower])*(position-lower)


def bootstrap_uncertainty(values):
    # Empirical bootstrap cannot infer unseen failures from an all-pass sample,
    # or infer disagreement from identical outcomes. A point-mass distribution
    # is not evidence of zero uncertainty, even after the full cohort finishes.
    degenerate = min(values) == max(values)
    return {'ci95': None if degenerate else [percentile(values, .025), percentile(values, .975)],
            'bootstrap_degenerate': degenerate, 'interval_informative': not degenerate,
            **({'interval_unavailable_reason': 'Constant bootstrap distribution; no informative uncertainty interval.'}
               if degenerate else {})}


def cluster_interval(pairs, agents, field, repetitions=10000, seed=SEED):
    clusters = defaultdict(list)
    for pair in pairs:
        clusters[pair[agents[0]]['exercise']].append(pair)
    sizes, sums = [], []
    for name in sorted(clusters):
        cases = clusters[name]
        sizes.append(len(cases))
        sums.append(tuple(sum(pair[agent][field] for pair in cases) for agent in agents))
    n = len(pairs)
    result = {'cases': n, 'exercise_clusters': len(sizes),
              'arms': {agent: {'rate': rate(sum(pair[agent][field] for pair in pairs), n),
                               'ci95': None} for agent in agents},
              'delta': {'direction': f'{agents[0]} minus {agents[1]}',
                        'rate': rate(sum(pair[agents[0]][field]-pair[agents[1]][field]
                                         for pair in pairs), n), 'ci95': None}}
    if len(sizes) < 2:
        result['interval_unavailable_reason'] = 'At least two completed exercise clusters are required.'
        for stats in [*result['arms'].values(), result['delta']]:
            stats.update(bootstrap_degenerate=None, interval_informative=False)
        return result
    rng = random.Random(seed)
    draws = [[] for _ in agents]
    differences = []
    for _ in range(repetitions):
        chosen = rng.choices(range(len(sizes)), k=len(sizes))
        count = sum(sizes[index] for index in chosen)
        scores = [sum(sums[index][arm] for index in chosen)/count for arm in range(len(agents))]
        for arm, score in enumerate(scores):
            draws[arm].append(score)
        differences.append(scores[0]-scores[1])
    for agent, values in zip(agents, draws):
        result['arms'][agent].update(bootstrap_uncertainty(values))
    result['delta'].update(bootstrap_uncertainty(differences))
    return result


def paired(rows, agents, repetitions, seed):
    by_case = defaultdict(dict)
    for row in rows:
        if row.get('evaluable', True) and row['status'] == 'recorded':
            by_case[row['case']][row['agent']] = row
    pairs = [by_case[name] for name in sorted(by_case) if all(a in by_case[name] for a in agents)]
    planned = len({row['case'] for row in rows if row.get('evaluable', True)})
    full = bool(planned) and len(pairs) == planned
    clean = [pair for pair in pairs if not any(pair[a]['infrastructure_failure'] for a in agents)]
    result = {'method': 'paired percentile bootstrap over exercise slugs; all language variants retained',
              'seed': seed, 'repetitions': repetitions,
              'planned_pairs': planned, 'completed_pairs': len(pairs), 'full_cohort_complete': full,
              'interval_scope': 'full_planned_paired_cohort' if full else 'completed_pairs_only',
              'warning': 'Partial cohorts are conditional on recorded cases; intervals are not 225-independent Wilson intervals.',
              'delta_direction': f'{agents[0]} minus {agents[1]}'}
    for name, cohort in [('all_completed_pairs', pairs), ('without_explicit_infrastructure', clean)]:
        result[name] = {'excluded_pairs': len(pairs)-len(cohort),
                        'first_pass': cluster_interval(cohort, agents, 'first_pass', repetitions, seed),
                        'final': cluster_interval(cohort, agents, 'passed', repetitions, seed),
                        'outcomes': dict(Counter(
                            'both_pass' if pair[agents[0]]['passed'] and pair[agents[1]]['passed'] else
                            agents[0]+'_only' if pair[agents[0]]['passed'] else
                            agents[1]+'_only' if pair[agents[1]]['passed'] else 'both_fail'
                            for pair in cohort))}
        for field in ('first_pass', 'final'):
            interval = result[name][field]
            interval['full_cohort_complete'] = full
            if not full:
                interval['interval_unavailable_reason'] = 'Full planned paired cohort is incomplete.'
                for stats in [*interval['arms'].values(), interval['delta']]:
                    # Preserve a conditional descriptive interval separately;
                    # it must never look like the full-benchmark interval.
                    stats['completed_cohort_ci95'] = stats['ci95']
                    stats['ci95'] = None
                    stats['interval_informative'] = False
    return result


def build_report(runroot, *, repetitions=10000, seed=SEED):
    if repetitions < 100:
        raise ValueError('Use at least 100 bootstrap repetitions')
    runroot = Path(runroot).resolve()
    manifest = json.loads((runroot/'run.json').read_text())
    cases = manifest['cases']
    agents = manifest.get('agents', ['proteus', 'opencode'])
    if len(agents) != 2 or len(set(agents)) != 2:
        raise ValueError('Paired report requires exactly two distinct agents')
    if len({case['id'] for case in cases}) != len(cases):
        raise ValueError('Duplicate case IDs in the frozen manifest')
    rows = [project(case, agent, runroot/'trials'/agent/case['id']/'result.json')
            for case in cases for agent in agents]
    languages = sorted({case['language'] for case in cases})
    summaries = {agent: summarize([row for row in rows if row['agent'] == agent]) for agent in agents}
    evaluable = [case for case in cases if case.get('evaluable', True) is True]
    nontrivial_rows = [row for row in rows if row['evaluable'] and row['baseline_pass'] is not True]
    return {'format': 1, 'runroot': str(runroot), 'conditions': manifest.get('conditions', {}),
            'corpus_cases': len(cases), 'planned_cases': len(evaluable),
            'excluded_cases': [{key: case.get(key) for key in ('id', 'language', 'exercise', 'evaluation_reason')}
                               for case in cases if case.get('evaluable', True) is not True],
            'baseline_pass_cases': sum(case.get('baseline_pass') is True for case in evaluable),
            'planned_exercise_clusters': len({case['exercise'] for case in evaluable}),
            'complete': all(summaries[agent]['complete'] for agent in agents),
            'agents': summaries,
            'per_language': {lang: {agent: summarize([row for row in rows
                if row['language'] == lang and row['agent'] == agent]) for agent in agents} for lang in languages},
            'paired': paired(rows, agents, repetitions, seed),
            'nontrivial': {'planned_cases': len(nontrivial_rows)//len(agents),
                          'agents': {agent: summarize([row for row in nontrivial_rows if row['agent'] == agent])
                                     for agent in agents},
                          'paired': paired(nontrivial_rows, agents, repetitions, seed)},
            'rows': rows}


def percent(value):
    return f'{value*100:.1f}%' if number(value) else '—'


def amount(value, precision=3):
    return f'{value:.{precision}f}' if number(value) else '—'


def markdown(report):
    lines = ['# Polyglot: Proteus и OpenCode', '',
             f"Корпус: {report.get('corpus_cases', report['planned_cases'])} задач; "
             f"оцениваются {report['planned_cases']}, {report['planned_exercise_clusters']} разных упражнений. "
             + ('Все прогоны завершены.' if report['complete'] else 'Прогон не завершён; отсутствующие результаты не считаются ошибками модели.'), '',
             '| Харнес | Выполнено | С первого раза | Финальный PASS | Доля среди выполненных | Стоимость, $ | $/успех | Cache hit | Среднее с/задачу |',
             '|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    for agent, data in report['agents'].items():
        lines.append(f"| {agent} | {data['attempted']}/{data['planned']} | {data['first_passes']} | "
                     f"{data['clean_successes']} | {percent(data['final_rate_observed'])} | "
                     f"{amount(data['cost_usd']['total'])} | {amount(data['cost_per_clean_success_usd'])} | "
                     f"{percent(data['cache_hit']['ratio'])} ({data['cache_hit']['known_trials']}/{data['attempted']}) | "
                     f"{amount(data['wall_seconds']['mean_seconds_observed'], 1)} |")
    lines += ['', 'PASS требует независимый grader и `pipeline_ok`. Стоимость включает неудачные попытки. '
              'Неизвестные usage не заменяются нулями; JSON содержит наблюдённые нижние границы. '
              'Стоимость является оценкой API-эквивалента, не подтверждённым списанием Go.', '',
              '## Парное сравнение', '',
              '95% интервалы полного прогона оцениваются bootstrap по названиям упражнений: языковые варианты одного '
              'упражнения остаются вместе, оба харнеса используют одну выборку. Это не 225 независимых задач.', '']
    if report.get('excluded_cases'):
        lines += ['Исключения определены upstream-контролями до прогона; они не считаются unrun или FAIL.']
        lines += [f"- {case['id']}: {case['evaluation_reason']}" for case in report['excluded_cases']]
        lines.append('')
    if report.get('baseline_pass_cases'):
        lines += [f"У {report['baseline_pass_cases']} оцениваемых задач исходная заготовка уже проходит tests. "
                  f"Они остаются в основном результате; дополнительный срез без них: "
                  f"{report['nontrivial']['planned_cases']} задач (JSON `nontrivial`).", '']
    for label, key in [('Все завершённые пары', 'all_completed_pairs'),
                       ('Без явно отмеченной инфраструктуры', 'without_explicit_infrastructure')]:
        data = report['paired'][key]['final']
        lines.append(f"**{label}:** {data['cases']} пар, {data['exercise_clusters']} кластеров.")
        show_intervals = report['paired']['full_cohort_complete']
        for agent, stats in data['arms'].items():
            interval = stats['ci95']
            text = f"- {agent}: наблюдаемая доля {percent(stats['rate'])}"
            if show_intervals:
                text += (f"; 95% CI [{percent(interval[0])}, {percent(interval[1])}]" if interval
                         else '; информативный интервал недоступен')
            lines.append(text+'.')
        interval = data['delta']['ci95']
        text = f"- Наблюдаемая Δ ({data['delta']['direction']}): {percent(data['delta']['rate'])}"
        if show_intervals:
            text += (f"; 95% CI [{percent(interval[0])}, {percent(interval[1])}]" if interval
                     else '; информативный интервал недоступен')
        lines.append(text+'.')
        lines.append('')
    if not report['paired']['full_cohort_complete']:
        lines += ['Прогон не завершён: доверительные интервалы полного бенчмарка пока не приводятся. '
                  'Наблюдаемые доли относятся только к завершённым парам.', '']
    lines += ['Постоянное bootstrap-распределение не даёт информативного интервала; '
              'оно не означает отсутствия неопределённости. '
              'Deadline и ошибки модели остаются в основном знаменателе. '
              'Инфраструктурные исключения показаны дополнительным срезом.', '',
              '## По языкам', '', '| Язык | Харнес | Выполнено | Первый PASS | Финальный PASS |',
              '|---|---|---:|---:|---:|']
    for language, agents in report['per_language'].items():
        for agent, data in agents.items():
            lines.append(f"| {language} | {agent} | {data['attempted']}/{data['planned']} | "
                         f"{data['first_passes']} | {data['clean_successes']} |")
    return '\n'.join(lines)+'\n'


def write_csv(path, rows):
    fields = ('agent', 'case', 'language', 'exercise', 'evaluable', 'evaluation_reason',
              'baseline_pass', 'status', 'pipeline_ok', 'first_pass',
              'passed', 'infrastructure_failure', 'failure_kind', 'usage_complete', 'usage_cost_usd',
              'observed_cost_usd_lower_bound', 'wall_seconds', 'trial_wall_seconds', 'model_requests',
              'primary_model_requests', 'all_tool_calls', 'turns', *TOKENS)
    with path.open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({key: row.get('tokens', {}).get(key) if key in TOKENS else row.get(key)
                             for key in fields})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', type=Path, required=True)
    parser.add_argument('--output', type=Path, help='JSON path; .md and .csv are written alongside')
    parser.add_argument('--bootstrap-repetitions', type=int, default=10000)
    parser.add_argument('--seed', type=int, default=SEED)
    args = parser.parse_args()
    report = build_report(args.runs, repetitions=args.bootstrap_repetitions, seed=args.seed)
    output = args.output or args.runs/'report.json'
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False)+'\n')
    output.with_suffix('.md').write_text(markdown(report))
    write_csv(output.with_suffix('.csv'), report['rows'])
    print(output)


if __name__ == '__main__':
    main()
