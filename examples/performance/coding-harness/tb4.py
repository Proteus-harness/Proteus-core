#!/usr/bin/env python3
"""Freeze and run the same small Terminal-Bench 4 pilot in native harnesses."""
import argparse
import asyncio
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import tomllib

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
SOURCE = CORE / 'target/performance/upstream/terminal-bench-4'
OUTPUT = CORE / 'target/performance/tb4-pilot'
PIN = '452bf305c6daa62fc59061d22133a7cbc7c1572e'
TASKS = ('bun-sourcemap-leak', 'mvcc-lsm-compaction', 'ontology-kg-querying',
         'photonic-waveguide-routing', 'react-lead-form', 'session-window-debug',
         'wal-recovery-ordering')
RULE = ('All Software tasks with cpus <= 2, memory_mb <= 4096, storage_mb <= 10240, '
        'no GPUs and no environment compose file; alphabetical order; no outcome filtering')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def hashes(path):
    return {str(p.relative_to(path)): sha(p) for p in sorted(path.rglob('*')) if p.is_file()}


def source_receipt():
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=SOURCE, text=True).strip()
    if revision != PIN:
        raise ValueError(f'TB4 revision differs: {revision}')
    if subprocess.check_output(['git', 'status', '--porcelain'], cwd=SOURCE, text=True):
        raise ValueError('TB4 checkout must be clean')
    tasks = []
    for name in TASKS:
        path = SOURCE / 'tasks' / name
        data = tomllib.loads((path / 'task.toml').read_text())
        env = data['environment']
        if not (data['metadata']['category'] == 'Software' and env['cpus'] <= 2
                and env['memory_mb'] <= 4096 and env['storage_mb'] <= 10240
                and not env.get('gpus') and not list((path / 'environment').glob('*compose*'))):
            raise ValueError(f'{name} violates fixed selection rule')
        if data['verifier']['environment_mode'] != 'separate':
            raise ValueError(f'{name} must retain separate upstream verification')
        tasks.append({'id': name, 'files': hashes(path), 'environment': env,
                      'agent': data['agent'], 'verifier': data['verifier'],
                      'artifacts': data['artifacts']})
    return {'format': 1, 'benchmark': 'Terminal-Bench 4.0 CPU Software pilot',
            'source': 'https://github.com/harbor-framework/terminal-bench',
            'revision': PIN, 'selection_rule': RULE, 'tasks': tasks,
            'published_reference': {
                'model': 'DeepSeek V4.1 Flash', 'effort': 'max',
                'score': 0.312, 'scaffold': 'DeepSeek Harness Minimal',
                'url': 'https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash',
                'comparable_to_pilot': False},
            'conditions': {'model': 'opencode-go/deepseek-v4.1-flash', 'effort': 'high',
                           'attempts_per_task': 1, 'timeout_seconds': 1200,
                           'disable_subagents': True, 'max_output_tokens': None,
                           'network': 'upstream public default',
                           'official_full_benchmark_score': False}}


def freeze():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    path = OUTPUT / 'manifest.json'
    receipt = source_receipt()
    if path.exists() and json.loads(path.read_text()) != receipt:
        raise ValueError('Existing frozen manifest differs; use a new pilot, never rewrite outcomes')
    path.write_text(json.dumps(receipt, indent=2) + '\n')
    print(path)


async def run(args):
    # Pin the orchestrator as well as the tasks. No inference credentials go in
    # JobConfig; each adapter privately exports just the active account entry.
    if importlib.metadata.version('harbor') != '0.26.0':
        raise ValueError('This pilot requires harbor==0.26.0')
    manifest = OUTPUT / 'manifest.json'
    if json.loads(manifest.read_text()) != source_receipt():
        raise ValueError('Frozen task manifest no longer matches the source')
    os.environ['HARBOR_TELEMETRY'] = '0'
    os.environ['LITELLM_LOCAL_MODEL_COST_MAP'] = 'True'
    os.environ.setdefault('DOCKER_HOST', f'unix:///run/user/{os.getuid()}/podman/podman.sock')
    from harbor.job import Job
    from harbor.models.job.config import JobConfig, RetryConfig
    from harbor.models.trial.config import AgentConfig, EnvironmentConfig, TaskConfig

    conditions = json.loads(manifest.read_text())['conditions']
    selected = args.only or list(TASKS)
    if any(name not in TASKS for name in selected) or len(set(selected)) != len(selected):
        raise ValueError('--only must name distinct tasks from the frozen manifest')
    if args.arm in ('oracle', 'nop'):
        agent = AgentConfig(name=args.arm, override_timeout_sec=1200)
    else:
        agent = AgentConfig(import_path='tb4_agent:ProteusAgent',
                            model_name=conditions['model'],
                            skip_model_parsing=True,
                            override_timeout_sec=conditions['timeout_seconds'] + 60,
                            override_setup_timeout_sec=900,
                            kwargs={'timeout': conditions['timeout_seconds'],
                                    'reasoning': conditions['effort'],
                                    'disable_subagents': conditions['disable_subagents'],
                                    'inspect_only': args.arm == 'inspect'})
    config = JobConfig(job_name=args.name, jobs_dir=OUTPUT / 'jobs',
                       agents=[agent], n_attempts=1, n_concurrent_trials=args.concurrency,
                       retry=RetryConfig(max_retries=0),
                       tasks=[TaskConfig(path=SOURCE / 'tasks' / name) for name in selected],
                       environment=EnvironmentConfig(type='podman', delete=True), quiet=True)
    receipt = {'arm': args.arm, 'tasks': selected, 'manifest_sha256': sha(manifest),
               'harbor_version': '0.26.0', 'conditions': conditions,
               'partial_control': selected != list(TASKS),
               'config': config.model_dump(mode='json')}
    receipt_path = OUTPUT / f'{args.name}.json'
    if receipt_path.exists() or (OUTPUT / 'jobs' / args.name).exists():
        raise ValueError('Job name already exists; keep failed attempts intact')
    receipt_path.write_text(json.dumps(receipt, indent=2) + '\n')
    job = await Job.create(config)
    result = await job.run()
    print(result.model_dump_json(indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('freeze')
    job = sub.add_parser('run')
    job.add_argument('--arm', choices=['oracle', 'nop', 'inspect', 'proteus'], required=True)
    job.add_argument('--name', required=True)
    job.add_argument('--only', action='append', choices=TASKS)
    job.add_argument('--concurrency', type=int, choices=[1, 2], default=2)
    args = parser.parse_args()
    if args.command == 'freeze':
        freeze()
    else:
        asyncio.run(run(args))


if __name__ == '__main__':
    main()
