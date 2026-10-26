#!/usr/bin/env python3
"""Measure actual AgentControl peers on synthetic local data, without model services."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import threading
import time

from proc import Tree, table

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def config(module, delay_ms):
    command = json.dumps(str(module))
    return f'''active_provider = "fake"
[providers.fake]
provider = "fake"
model = "fake-tool-model"
stream = true
[modules]
workflow = "coding.codex_loop"
context = "codex_context"
policy = "allow_all"
hooks = ["codex_compact"]
[components.reference-model]
command = {command}
[components.reference-model.exports.model.fake]
[components.reference-workflow]
command = {command}
[components.reference-workflow.exports.workflow."coding.codex_loop"]
[components.reference-context]
command = {command}
[components.reference-context.exports.context.codex_context]
[components.reference-capabilities]
command = {command}
[components.reference-capabilities.exports.policy.allow_all]
[components.reference-capabilities.exports.hook.codex_compact]
[components.reference-capabilities.exports.tool.file_tools]
[module_config.model.fake]
implementation = "fake"
stream_delay_ms = {delay_ms}
[module_config.context.codex_context]
providers = ["environment"]
max_context_bytes = 60000
[module_config.hook.codex_compact]
trigger_tokens = 10000000
[tools]
enabled = ["read_file"]
[agent_control]
surface = "none"
roles = []
[event_log]
path = ".proteus/events.jsonl"
'''


def verified_journals(binary, work, env, peers, rounds):
    journals = sorted((work/'config/sessions').rglob('journal.jsonl'))
    assert len(journals) == peers, f'{len(journals)} journals, expected {peers}'
    result = []
    for index, journal in enumerate(journals):
        before = digest(journal)
        records = [json.loads(line) for line in journal.read_text().splitlines()]
        settled = [r for r in records if r.get('kind') == 'turn_settled']
        assert len(settled) == rounds, f'{len(settled)} settled turns, expected {rounds}'
        for turn_index, record in enumerate(settled):
            command = [str(binary), '--config', str(work/'config/configs/peer.toml'),
                       '--cwd', str(work/'workspace'), 'replay', 'workflow', str(journal),
                       '--turn-id', record['turn_id'], '--json']
            replay = subprocess.run(command, cwd=work/'workspace', env=env,
                                    capture_output=True, text=True, timeout=120)
            (work/f'replay-{index}-{turn_index}.log').write_text(replay.stdout+replay.stderr)
            assert replay.returncode == 0, f'replay failed: {replay.stderr[-1500:]}'
            payload = json.loads(replay.stdout)
            assert payload['comparison']['matched'], payload
            assert payload['source_journal_unchanged'] and before == digest(journal)
        result.append(dict(bytes=journal.stat().st_size, sha256=before,
                           settled_turns=len(settled), replay_matched=True))
    return result


def model_overlap(work, peers):
    records = [json.loads(line) for line in (work/'config/.proteus/events.jsonl').read_text().splitlines()]
    active = set()
    maximum = 0
    calls = 0
    # timestamp_ms is wall-clock time shared by the local processes. Preserve
    # append order for ties; each individual peer's request/response order holds.
    for record in sorted(records,key=lambda record:record['timestamp_ms']):
        key = (record['session_id'],record['thread_id'])
        if 'ModelRequestPrepared' in record['event']:
            assert key not in active, 'overlapping requests inside one sequential fake peer'
            active.add(key)
            calls += 1
            maximum = max(maximum,len(active))
        elif 'ModelResponseReceived' in record['event']:
            assert key in active, 'response without a request'
            active.remove(key)
    assert not active and maximum == peers, f'only {maximum}/{peers} concurrent model calls'
    return dict(maximum_in_flight=maximum,model_calls=calls)


def cleanup(process, tree):
    tree.sample()
    for force in (False,True):
        current = table()
        alive = [pid for pid,start in tree.owned.items()
                 if pid in current and current[pid]['start_ticks']==start]
        if not alive:
            break
        for pid in reversed(alive):
            try: os.kill(pid,signal.SIGKILL if force else signal.SIGTERM)
            except ProcessLookupError: pass
        if process.poll() is None:
            try: os.killpg(process.pid,signal.SIGKILL if force else signal.SIGTERM)
            except ProcessLookupError: pass
        try: process.wait(timeout=5)
        except subprocess.TimeoutExpired: pass
        time.sleep(.1)


def measure(args, peers, repeat):
    work = args.output/f'peers-{peers}-repeat-{repeat}'
    assert not work.exists(), f'refusing to overwrite evidence: {work}'
    workspace = work/'workspace'
    configs = work/'config/configs'
    workspace.mkdir(parents=True)
    configs.mkdir(parents=True)
    (work/'home').mkdir()
    (work/'xdg').mkdir()
    payload = ('SYNTHETIC_PAYLOAD_'+('0123456789abcdef'*1024*args.payload_kib))[:args.payload_kib*1024]
    (workspace/'payload.txt').write_text(payload)
    (workspace/'AGENTS.md').write_text('Synthetic local resource benchmark.\n')
    profile = configs/'peer.toml'
    profile.write_text(config(args.module, args.delay_ms))
    idle_cap = peers if args.idle_cap is None else args.idle_cap
    driver_input = dict(binary=str(args.binary),config=str(profile),workspace=str(workspace),
                        peers=peers,rounds=args.rounds,idle_cap=idle_cap)
    path = work/'input.json'
    path.write_text(json.dumps(driver_input,indent=2)+'\n')
    env = os.environ.copy()
    env.update(PROTEUS_CONFIG_HOME=str(work/'config'), PROTEUS_HOME=str(work/'home'),
               HOME=str(work/'home'),
               XDG_CONFIG_HOME=str(work/'xdg'), PROTEUS_TEST_REFERENCE_MODULE=str(args.module),
               NO_PROXY='localhost,127.0.0.1,::1', no_proxy='localhost,127.0.0.1,::1')
    events = []
    with (work/'driver.stderr.log').open('w') as error:
        process = subprocess.Popen([str(args.driver),str(path)],cwd=workspace,env=env,
                                   stdout=subprocess.PIPE,stderr=error,text=True,start_new_session=True)
        tree = Tree(process.pid)

        def read_events():
            with (work/'events.jsonl').open('w') as output:
                for line in process.stdout:
                    output.write(line)
                    output.flush()
                    events.append(json.loads(line))

        reader = threading.Thread(target=read_events,daemon=True)
        reader.start()
        start = time.monotonic()
        samples = []
        try:
            while process.poll() is None:
                samples.append(dict(elapsed_s=time.monotonic()-start,
                                    phase=events[-1]['phase'] if events else 'launch',**tree.sample()))
                if samples[-1]['pss_swap_kib'] > args.max_pss_mib*1024:
                    raise MemoryError(f'sampled PSS+SwapPss exceeded {args.max_pss_mib} MiB')
                if time.monotonic()-start > args.timeout:
                    raise TimeoutError('benchmark driver deadline exceeded')
                time.sleep(args.interval)
            reader.join(timeout=5)
            assert process.returncode == 0, (work/'driver.stderr.log').read_text()[-3000:]
            current = table()
            assert not any(pid in current and current[pid]['start_ticks']==ticks
                           for pid,ticks in tree.owned.items()), 'owned descendants survived driver teardown'
        finally:
            cleanup(process,tree)
            (work/'samples.json').write_text(json.dumps(samples,indent=2)+'\n')
    assert any(e['phase']=='done' for e in events), 'missing driver completion'
    last = tree.sample()
    assert not last['processes'], 'owned descendants survived driver teardown'
    verification = verified_journals(args.binary,work,env,peers,args.rounds)
    overlap = model_overlap(work,peers)
    assert overlap['model_calls']==peers*args.rounds*2, overlap
    resident = [s['pss_swap_kib'] for s in samples if s['phase']=='resident_idle']
    teardown = [s['pss_swap_kib'] for s in samples if s['phase']=='teardown']
    result = dict(peers=peers,repeat=repeat,idle_cap=idle_cap,rounds=args.rounds,
        payload_bytes=len(payload.encode()),sampled_peak_pss_swap_mib=max(s['pss_swap_kib'] for s in samples)/1024,
        resident_pss_swap_mib=statistics.median(resident)/1024 if resident else None,
        teardown_pss_swap_mib=statistics.median(teardown)/1024 if teardown else None,
        elapsed_seconds=events[-1]['elapsed_ms']/1000,io_and_cpu=tree.totals(),
        events=events,samples=samples,journals=verification,model_overlap=overlap,status='passed')
    (work/'report.json').write_text(json.dumps(result,indent=2)+'\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--driver',type=Path,default=ROOT/'target/release/examples/multiagent_perf')
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--module',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--counts',type=int,nargs='+',default=[1,5,10])
    parser.add_argument('--rounds',type=int,default=3)
    parser.add_argument('--repeats',type=int,default=1)
    parser.add_argument('--payload-kib',type=int,default=32)
    parser.add_argument('--delay-ms',type=int,default=500)
    parser.add_argument('--idle-cap',type=int)
    parser.add_argument('--interval',type=float,default=.2)
    parser.add_argument('--timeout',type=int,default=180)
    parser.add_argument('--max-pss-mib',type=int,default=4096)
    args = parser.parse_args()
    for key in ('driver','binary','module'):
        path = getattr(args,key).resolve()
        assert path.is_file(),path
        setattr(args,key,path)
    args.output=args.output.resolve()
    assert all(1<=count<=10 for count in args.counts)
    assert 1<=args.rounds<=20 and 1<=args.payload_kib<=128 and args.repeats>=1
    assert args.interval>=.05 and 0<args.timeout<=600 and 0<args.max_pss_mib<=8192
    assert 0<args.delay_ms<=5000 and (args.idle_cap is None or 0<=args.idle_cap<=10)
    args.output.mkdir(parents=True,exist_ok=True)
    manifest={key:dict(path=str(getattr(args,key)),sha256=digest(getattr(args,key))) for key in ('driver','binary','module')}
    (args.output/'binaries.json').write_text(json.dumps(manifest,indent=2)+'\n')
    results=[]
    for repeat in range(args.repeats):
        for peers in args.counts:
            result=measure(args,peers,repeat)
            results.append({key:value for key,value in result.items() if key not in ('events','samples')})
            print(json.dumps({key:result[key] for key in ('peers','repeat','sampled_peak_pss_swap_mib','resident_pss_swap_mib','teardown_pss_swap_mib')},indent=2),flush=True)
    report=dict(status='passed',binaries=manifest,runs=results,
        scope='Parent AgentControl control plane plus real Core stdio peers; no GUI/root inference, fake model, real Rust workflow/context/hook/file tool. Warm OS cache. Sampled peaks and last-seen /proc counters are lower bounds, not exact peaks or allocation counts. IO wchar includes protocol traffic as well as file writes.')
    (args.output/'report.json').write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':
    main()
