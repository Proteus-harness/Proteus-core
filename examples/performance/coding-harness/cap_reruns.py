#!/usr/bin/env python3
"""Three separate fresh Codex reruns after removing the implicit output cap."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import adapters
import corpus
import limit_control
import reconnect_runs as reconnect
import runner

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
DATA = CORE/'target/performance/coding-harness'
PREVIOUS = DATA/'reconnect-2026-11-12-01/fresh'
CASES = ('schemelike-metacircular-eval','journal-segments','ledger-recovery')
MODEL = reconnect.MODEL


def sources():
    return {case:PREVIOUS/f'flash_{case}_1_proteus-codex' for case in CASES}


def frozen_sources():
    return {path.name:runner.tree_manifest(path) for path in sorted(PREVIOUS.iterdir()) if path.is_dir()}


def prepare(output):
    output.mkdir(parents=True,exist_ok=False,mode=0o700)
    rows = []
    for case,source in sources().items():
        result=json.loads((source/'result.json').read_text())
        assert result['model']==MODEL and result['reasoning']=='high'
        assert 'length limit' in json.dumps(result['metrics']['failures'])
        rows.append({'case':case,'arm':'proteus-codex','attempt':1,
                     'baseline':reconnect.relative(source),'baseline_caps':[16384]})
    old=json.loads((CORE/'examples/performance/results/2026-11-11-deepseek-long.json').read_text())
    reconnect.write_json(output/'selection.json',{'rows':rows,'model':MODEL,'effort':'high',
                         'timeout_seconds':1200,'concurrency':3,'max_attempts_per_cell':1,
                         'old_controls':[{'case':row['case'],'source':row['source'],
                                          'caps':row['audit']['requested_output_limits'],
                                          'failure_taxonomy':row['failure_taxonomy']}
                                         for row in old['output_cap_controls']]})
    reconnect.write_json(output/'frozen-sources-before.json',frozen_sources())


def catalog_snapshot():
    module=CORE.parent/'Proteus-desktop/build/Proteus/share/pi-runtime/tool/pi-tools/node_modules/@earendil-works/pi-coding-agent/dist/core/model-runtime.js'
    script=('import { ModelRuntime } from '+json.dumps(module.as_uri())+'; '
            'const r=await ModelRuntime.create({modelsPath:null,allowModelNetwork:false}); '
            'const m=r.getModel("opencode-go","deepseek-v4.1-flash"); '
            'console.log(JSON.stringify({id:m.id,provider:m.provider,maxTokens:m.maxTokens,contextWindow:m.contextWindow,api:m.api}));')
    result=subprocess.run(['node','--input-type=module','-e',script],check=True,capture_output=True,text=True)
    value=json.loads(result.stdout)
    assert value['id']=='deepseek-v4.1-flash' and value['provider']=='opencode-go' and value['maxTokens']==384000
    return {'model':value,'module':str(module),'module_sha256':runner.digest(module),
            'network_model_refresh':False,'model_calls':0}


def observer_selfcheck():
    with tempfile.TemporaryDirectory(prefix='proteus-cap-observer-')as temporary:
        base=Path(temporary);workspace=base/'workspace';runtime=base/'runtime'
        workspace.mkdir();runtime.mkdir()
        script='''
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const body=JSON.stringify({model:'deepseek-v4.1-flash',reasoning_effort:'high',max_tokens:384000});
const options={method:'POST',body};
const response=new Response('unchanged-stream-content');
let observed;
globalThis.fetch=async (...args)=>{observed=args;return response;};
await import('/runtime/cap_wire_observer.mjs');
const result=await fetch('https://opencode.ai/inference/go/openai/v1/chat/completions',options);
assert.equal(result,response);assert.equal(observed[1],options);assert.equal(options.body,body);
assert.equal(await result.text(),'unchanged-stream-content');
const rows=readFileSync('/state/cap-wire.jsonl','utf8').trim().split('\\n').map(JSON.parse);
assert.equal(rows[0].max_tokens,384000);assert.equal(rows[0].reasoning_effort,'high');
assert.equal(rows[1].status,200);
'''
        command=runner.sandbox_command(['node','--input-type=module','-e',script],
                    [(HERE/'cap_wire_observer.mjs','/runtime/cap_wire_observer.mjs')],workspace,runtime)
        command.insert(1,'--unshare-net')
        subprocess.run(command,check=True,capture_output=True,text=True)
    print('Observer kept request identity, body and response unchanged; network disabled')


def worker(case,root):
    """Reuse runner unchanged; add a metadata-only observer to its provider component."""
    output=root/f'flash_{case}_1_proteus-codex'
    original=adapters.prepare
    def observed_prepare(args,runtime):
        inner,mounts=original(args,runtime)
        observer=runtime/'executed-cap_wire_observer.mjs'
        shutil.copy2(HERE/'cap_wire_observer.mjs',observer)
        config_path=runtime/'configs/eval.json'
        config=json.loads(config_path.read_text())
        component=config['components']['pi-benchmark-model']
        component['env_allowlist'].append('NODE_OPTIONS')
        component.setdefault('env',{})['NODE_OPTIONS']='--import /runtime/cap_wire_observer.mjs'
        reconnect.write_json(config_path,config)
        mounts.append((observer,'/runtime/cap_wire_observer.mjs'))
        reconnect.write_json(runtime/'observer-provenance.json',{
            'executed_sha256':runner.digest(observer),'mode':'read-only fetch metadata; unchanged request/response'})
        return inner,mounts
    adapters.prepare=observed_prepare
    sys.argv=[str(HERE/'runner.py'),'--agent','proteus',
              '--binary',str(CORE.parent/'Proteus-desktop/build/Proteus/bin/proteus'),
              '--base-config',str(CORE/'configs/codex.config.toml'),
              '--case-dir',str(DATA/'long-cases'/case),'--output',str(output),
              '--model',MODEL,'--reasoning','high','--timeout','1200']
    try:
        runner.main()
    finally:
        adapters.prepare=original
    corpus.grade(DATA/'long-cases'/case,output/'workspace',output/'grading.json')


def evidence(case,root):
    output=root/f'flash_{case}_1_proteus-codex'
    result=json.loads((output/'result.json').read_text())
    grade=json.loads((output/'grading.json').read_text())
    baseline=json.loads((sources()[case]/'result.json').read_text())
    current=limit_control.journal_requests(output)
    previous=limit_control.journal_requests(sources()[case])
    wire_path=output/'runtime/cap-wire.jsonl'
    events=[json.loads(line)for line in wire_path.read_text().splitlines()]if wire_path.exists()else[]
    wire=[row for row in events if row['event']=='request']
    caps=[request['limits']['max_output_tokens']for request in current]
    same={'prompt':result['prompt_sha256']==baseline['prompt_sha256'],
          'fixture':result['fixture_files_sha256']==baseline['fixture_files_sha256'],
          'model_effort':(result['model'],result['reasoning'])==(baseline['model'],baseline['reasoning']),
          'initial_request_except_cap_and_ids':bool(current and previous and
              limit_control.initial_fields(current[0])==limit_control.initial_fields(previous[0]))}
    catalog=json.loads((root/'catalog-snapshot.json').read_text())['model']
    observed_wire_caps=[row.get('max_tokens')or row.get('max_completion_tokens')for row in wire]
    wire_selection=bool(wire and all(row['model']=='deepseek-v4.1-flash'and row['reasoning_effort']=='high'for row in wire))
    cap_removed=bool(caps and all(cap is None or cap==catalog['maxTokens']for cap in caps)
                     and wire and all(cap is None or cap==catalog['maxTokens']for cap in observed_wire_caps))
    row={'case':case,'source':reconnect.relative(output),'baseline':reconnect.relative(sources()[case]),
         'pipeline_ok':result['pipeline_ok'],'grade':grade['passed'],
         'pass':result['pipeline_ok']and grade['passed'],'seconds':result['wall_seconds'],
         'timed_out':result.get('timed_out'), 'model_verified':result.get('model_verified'),
         'reasoning_verified':result.get('reasoning_verified'),'auth_removed':result.get('auth_removed'),
         'requests':result['metrics']['model_requests'],'actions':result['metrics']['all_tool_calls'],
         'same_conditions':same,'canonical_caps':caps,'wire_caps':observed_wire_caps,
         'wire_selection_verified':wire_selection,'catalog_max_tokens':catalog['maxTokens'],
         'wire_cap_omitted':bool(wire and all(cap is None for cap in observed_wire_caps)),
         'wire_server_limit_known':False,
         'cap_removed_verified':cap_removed,'wire_events':events,
         'model_failures':[x for x in result['metrics'].get('failures',[])if x.get('kind')=='model'],
         'trace':reconnect.trace_evidence(output),**reconnect.usage_summary(result,result['metrics'])}
    row['verification_ok']=bool(all(same.values())and wire_selection and cap_removed
                                and row['model_verified']and row['reasoning_verified'])
    reconnect.write_json(output/'cap-evidence.json',row)
    return row


def report(root,destination):
    rows=[evidence(case,root)for case in CASES]
    data={'model':MODEL,'effort':'high','timeout_seconds':1200,'concurrency':3,
          'source':reconnect.relative(root),'rows':rows,
          'catalog':json.loads((root/'catalog-snapshot.json').read_text()),
          'runtime':json.loads((root/'runtime-unchanged.json').read_text()),
          'frozen_sources':json.loads((root/'frozen-sources-unchanged.json').read_text()),
          'selection':json.loads((root/'selection.json').read_text())}
    reconnect.write_json(destination.with_suffix('.json'),data)
    yes=lambda value:'да'if value else'нет'
    lines=['# Повторы Codex после снятия ограничения ответа','',
           'Три отдельные чистые попытки на DeepSeek V4.1 Flash/high, по 20 минут. '
           'Исходные десять запусков сохранены. Новые траектории случайны; это не парное воспроизведение ответа.', '',
           '| Задача | Процесс завершён | Тесты пройдены | Время | Действия | Запросы | Лимит исправлен |',
           '|---|---|---|---:|---:|---:|---|']
    for row in rows:
        duration=f"{round(row['seconds']):,}".replace(',',' ')
        lines.append(f"| {row['case']} | {yes(row['pipeline_ok'])} | {yes(row['grade'])} | {duration} с | "
                     f"{row['actions']} | {row['requests']} | {yes(row['cap_removed_verified'])} |")
    lines+=['',f"Процесс и тесты прошли {sum(row['pass']for row in rows)}/3. "
            'Проверены canonical requests и реальные HTTP request fields через наблюдатель, '
            'который сохраняет только метаданные и не изменяет запросы, ответы или streams.', '',
            'Каталог установленного Pi ModelRuntime сообщает 384 000 токенов ответа. '
            'Отсутствующее поле wire cap означает отсутствие клиентского ограничения; '
            'фактический максимум сервера этим не доказан. Полные значения и ошибки записаны в JSON.', '',
            'Прежние cap-controls для journal и ledger уже меняли canonical cap на 384 000, '
            'но оба завершились обрывом сокета. Тогда реальный wire не наблюдался, '
            'поэтому те проверки не подтверждают результат нынешнего исправления.', '',
            f"Снимок программы/runtime сохранился: {yes(data['runtime']['unchanged'])}. "
            f"Исходные результаты сохранились: {yes(data['frozen_sources']['unchanged'])}."]
    destination.with_suffix('.md').write_text('\n'.join(lines)+'\n')
    return data


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=DATA/'codex-cap-2026-11-12-01')
    parser.add_argument('--run',action='store_true')
    parser.add_argument('--selfcheck',action='store_true')
    parser.add_argument('--worker',choices=CASES)
    parser.add_argument('--report',type=Path)
    args=parser.parse_args();root=args.output.resolve()
    if args.selfcheck:observer_selfcheck();return 0
    if args.worker and not args.run:
        parser.error('Worker also requires explicit --run')
    if args.worker:
        worker(args.worker,root);return 0
    if args.report:
        result=report(root,args.report.resolve())
        return 0 if all(row['verification_ok']for row in result['rows'])else 1
    if not root.exists():prepare(root)
    if not args.run:
        print('Prepared without credentials or model calls',root);return 0
    if any(root.glob('flash_*')):raise ValueError('Refusing additional attempts in an existing run')
    if frozen_sources()!=json.loads((root/'frozen-sources-before.json').read_text()):
        raise ValueError('Frozen original receipts changed after preparation')
    reconnect.write_json(root/'runtime-snapshot.json',reconnect.runtime_snapshot())
    reconnect.write_json(root/'catalog-snapshot.json',catalog_snapshot())
    def launch(case):
        with (root/(case+'.runner.log')).open('w')as log:
            subprocess.run([sys.executable,'-B',str(HERE/'cap_reruns.py'),'--run','--output',str(root),'--worker',case],
                           cwd=CORE,stdout=log,stderr=subprocess.STDOUT,check=True)
        return evidence(case,root)
    with ThreadPoolExecutor(3)as pool:list(pool.map(launch,CASES))
    after=reconnect.runtime_snapshot()
    reconnect.write_json(root/'runtime-unchanged.json',{'unchanged':after==json.loads((root/'runtime-snapshot.json').read_text()),'after':after})
    reconnect.write_json(root/'frozen-sources-unchanged.json',{'unchanged':frozen_sources()==json.loads((root/'frozen-sources-before.json').read_text())})
    return 0


if __name__=='__main__':raise SystemExit(main())
