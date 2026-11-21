"""Score paired harness conditions from recorded output and the independent oracle."""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
from statistics import median

from usage_report import project_usage, summarize_usage


def read_rows(directories, excluded):
    rows = []
    identities = set()
    for directory in directories:
        for file in sorted(directory.glob('*/result.json')):
            model, case, repeat, arm = file.parent.name.split('_')
            result = json.loads(file.read_text())
            if arm == "opencode" and result["model"] == "opencode-go/deepseek-v4.1-flash":
                registry = result.get("metrics", {}).get("registry", {})
                if not registry.get("auxiliary_model_verified"):
                    excluded.append({"source": str(file.resolve()), "reason": "unverified auxiliary title model"})
                    continue
            key = model, case, repeat, arm
            if key in identities:
                raise ValueError(f'duplicate run {key}')
            identities.add(key)
            grade = json.loads((file.parent/'grading.json').read_text())
            metrics = result['metrics']
            row = {'model':result['model'], 'effort':result['reasoning'], 'case':case,
                         'attempt':int(repeat), 'arm':arm, 'source':str(file.resolve()),
                         'prompt_sha256':result['prompt_sha256'],
                         'repo_sha256':hashlib.sha256(json.dumps(result['fixture_files_sha256'],sort_keys=True).encode()).hexdigest(),
                         'pipeline_ok':result['pipeline_ok'], 'grade':grade['passed'],
                         'pass':result['pipeline_ok'] and grade['passed'], 'seconds':result['wall_seconds'],
                         'returncode':result.get('returncode'), 'timed_out':result.get('timed_out', False),
                         'failures':metrics.get('failures', metrics.get('errors', [])),
                         'actions':metrics.get('all_tool_calls'), 'tools':metrics.get('tool_names', {}),
                         'requests':metrics.get('model_requests')}
            row.update(project_usage(file.parent, result, metrics))
            rows.append(row)
    return rows


def complete_sum(values):
    return sum(values) if values and all(v is not None for v in values) else None


def summarize(rows):
    by_config = defaultdict(list)
    inputs = defaultdict(set)
    for row in rows:
        inputs[row['case']].add((row['prompt_sha256'],row['repo_sha256']))
        by_config[row['model'],row['effort'],row['arm']].append(row)
    if any(len(variants)!=1 for variants in inputs.values()):
        raise ValueError('fixture or prompt differs between agents')
    stats = []
    for (model,effort,arm),runs in sorted(by_config.items()):
        primary = [r for r in runs if r['attempt']==1]
        usage = summarize_usage(primary)
        tools = Counter()
        for r in primary:
            tools.update(r['tools'])
        stats.append({'model':model,'effort':effort,'arm':arm,
                      'primary_attempts':len(primary),'primary_passed':sum(r['pass'] for r in primary),
                      'repeat_attempts':len(runs)-len(primary),'repeat_passed':sum(r['pass'] for r in runs if r['attempt']>1),
                      'input':usage['tokens']['input_tokens'],
                      'cached':usage['tokens']['cached_input_tokens'],
                      'output':usage['tokens']['output_tokens'],
                      'known_token_runs':usage['known_token_runs'],
                      'observed_token_sums':usage['observed_token_sums'],
                      'requests':complete_sum([r['requests'] for r in primary]),
                      'actions':complete_sum([r['actions'] for r in primary]),'tools':dict(tools),
                      'median_wall_seconds':round(median(r['seconds'] for r in primary),2)})
    return stats


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directories',type=Path,nargs='+')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    excluded=[]
    rows=read_rows(args.directories, excluded)
    stats=summarize(rows)
    args.output.write_text(json.dumps({'rows':rows,'configurations':stats,'excluded':excluded},indent=2)+'\n')
    for s in stats:
        print(s['model'],s['effort'],s['arm'],str(s['primary_passed'])+'/'+str(s['primary_attempts']),
              s['actions'],s['input'],s['cached'],s['output'],s['requests'])
if __name__=='__main__':
    main()
