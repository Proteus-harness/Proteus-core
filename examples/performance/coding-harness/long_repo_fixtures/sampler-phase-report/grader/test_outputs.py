import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import math
import pytest

ROOT = Path('/app')
CLI = ROOT/'examples/performance/summarize.py'


def load():
    path=ROOT/'examples/performance/common/phase_report.py'
    spec=importlib.util.spec_from_file_location('phase_under_test',path)
    module=importlib.util.module_from_spec(spec)
    sys.modules[spec.name]=module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        sys.modules.pop(spec.name, None)
        raise
    return module.summarize


def sample(seq, phase='work', rss=100, pss=60, duration=1, cpu=100, identities=None):
    return dict(type='sample',sequence=seq,monotonic_ns=(seq+1)*1000000000,phase=phase,rss_kib=rss,pss_swap_kib=pss,
        interval_seconds=duration,interval_cpu_percent_one_core=cpu,memory_incomplete_pids=[],
        processes=[dict(pid=pid,start_ticks=start) for pid,start in (identities or [(10,1)])])


def write(path, rows, tail=b''):
    path.write_bytes(b''.join(json.dumps(row,ensure_ascii=False).encode()+b'\n' for row in rows)+tail)
    return path


def phases(result):return {row['phase']:row for row in result['phases']}


def test_weighted_cpu_phase_transitions_and_memory(tmp_path):
    a=sample(0,'boot',rss=10,pss=4,cpu=999)
    b=sample(1,'work',rss=30,pss=9,duration=10,cpu=900)
    c=sample(2,'work',rss=70,pss=30,duration=2,cpu=200,identities=[(10,1),(11,2)])
    d=sample(3,'work',rss=50,pss=20,duration=4,cpu=50,identities=[(10,9)])
    d['memory_incomplete_pids']=[10]
    out=load()([write(tmp_path/'one',[{'type':'phase','phase':'boot'},a,b,c,d,{'type':'summary'}])])
    assert out['runs']==1 and out['ignored_partial_lines']==0
    assert [r['phase'] for r in out['phases']]==['boot','work']
    p=phases(out)
    assert p['boot']==dict(phase='boot',samples=1,runs=1,median_rss_kib=10,median_pss_swap_kib=4,peak_rss_kib=10,
        peak_pss_swap_kib=4,cpu_seconds=0,cpu_percent_one_core=None,observed_processes=1,incomplete_memory_samples=0)
    work=p['work']
    assert work['samples']==3 and work['runs']==1
    assert work['median_rss_kib']==50 and work['median_pss_swap_kib']==20
    assert work['peak_rss_kib']==70 and work['peak_pss_swap_kib']==30
    assert work['cpu_seconds']==pytest.approx(6)
    assert work['cpu_percent_one_core']==pytest.approx(100)
    assert work['observed_processes']==3 and work['incomplete_memory_samples']==1


def test_independent_runs_duplicates_and_missing_phases(tmp_path):
    one=write(tmp_path/'a',[sample(7,None,rss=10),sample(8,None,rss=30)])
    two=write(tmp_path/'b',[sample(0,None,rss=100),sample(1,'z',rss=3),sample(2,'z',rss=7,cpu=300)])
    out=load()([one,str(one),one.parent/'.'/one.name,two])
    assert out['runs']==2
    assert [x['phase'] for x in out['phases']]==[None,'z']
    null,z=out['phases']
    assert null['runs']==2 and null['observed_processes']==2 and null['samples']==3
    assert null['median_rss_kib']==30 and null['cpu_seconds']==1
    assert z['cpu_seconds']==3 and z['cpu_percent_one_core']==300


@pytest.mark.parametrize('value',[None,True,False,-1,0,float('nan'),float('inf'),'5'])
def test_optional_invalid_duration_is_skipped(tmp_path,value):
    one=write(tmp_path/'a',[sample(0),sample(1,duration=value),sample(2,duration=2,cpu=75)])
    out=load()([one])['phases'][0]
    assert out['cpu_seconds']==1.5 and out['cpu_percent_one_core']==75


@pytest.mark.parametrize('value',[None,True,False,-1,float('nan'),float('inf'),'5'])
def test_optional_invalid_cpu_is_skipped(tmp_path,value):
    one=write(tmp_path/'a',[sample(0),sample(1,cpu=value)])
    out=load()([one])['phases'][0]
    assert out['cpu_seconds']==0 and out['cpu_percent_one_core'] is None


def test_sequence_gaps_and_runs_dont_bridge_cpu(tmp_path):
    one=write(tmp_path/'a',[sample(10,duration=10,cpu=900),sample(12,duration=10,cpu=800),sample(13,duration=3,cpu=150)])
    two=write(tmp_path/'b',[sample(5,duration=10,cpu=600)])
    out=load()([one,two])['phases'][0]
    assert out['cpu_seconds']==4.5 and out['cpu_percent_one_core']==150


@pytest.mark.parametrize('field,value',[('sequence',True),('sequence',-1),('monotonic_ns',False),('monotonic_ns',-1),('phase',''),
    ('phase',5),('rss_kib',True),('rss_kib',float('nan')),('pss_swap_kib',-1),('processes',{}),('memory_incomplete_pids',None)])
def test_required_metadata_validation(tmp_path,field,value):
    bad=sample(0);bad[field]=value
    with pytest.raises(ValueError):load()([write(tmp_path/'a',[bad])])


@pytest.mark.parametrize('field,value',[('pid',0),('pid',True),('pid',-1),('start_ticks',-1),('start_ticks',False)])
def test_pid_identity_validation(tmp_path,field,value):
    bad=sample(0);bad['processes'][0][field]=value
    with pytest.raises(ValueError):load()([write(tmp_path/'a',[bad])])


@pytest.mark.parametrize('kind',['duplicate-sequence','backward-time','unknown','blank','bad-json','missing-field'])
def test_malformed_streams_raise(tmp_path,kind):
    data=[sample(0),sample(1)]
    if kind=='duplicate-sequence':data[1]['sequence']=0
    if kind=='backward-time':data[1]['monotonic_ns']=data[0]['monotonic_ns']
    if kind=='unknown':data.append({'type':'strange'})
    if kind=='missing-field':del data[1]['rss_kib']
    p=write(tmp_path/'a',data)
    if kind=='blank':p.write_bytes(p.read_bytes()+b'\n')
    if kind=='bad-json':p.write_bytes(p.read_bytes()+b'{bad}\n')
    with pytest.raises(ValueError):load()([p])


def test_partial_tail_and_empty_marker_only(tmp_path):
    one=write(tmp_path/'a',[sample(0)],b'{"unfinished":')
    two=write(tmp_path/'b',[{'type':'phase','phase':'empty'},{'type':'summary'}],b'{"type":"summary"}')
    out=load()([one,two])
    assert out['runs']==2 and out['ignored_partial_lines']==2 and len(out['phases'])==1
    assert load()([])==dict(runs=0,ignored_partial_lines=0,phases=[])
    assert load()([two])['phases']==[]


def test_aggregator_streams_file(tmp_path,monkeypatch):
    path=write(tmp_path/'a',[sample(i) for i in range(500)])
    f=load()
    def denied(*args,**kwargs):raise AssertionError('whole-file read forbidden')
    monkeypatch.setattr(Path,'read_text',denied);monkeypatch.setattr(Path,'read_bytes',denied)
    assert f([path])['phases'][0]['samples']==500


def run_cli(tmp_path,*args):
    return subprocess.run([sys.executable,str(CLI),*map(str,args)],cwd=tmp_path,capture_output=True,text=True)


def test_cli_samples_only_and_escaping(tmp_path):
    before=write(tmp_path/'a',[sample(0,'work|x\ny',rss=10),sample(1,'work|x\ny',rss=30,cpu=200)])
    after=write(tmp_path/'b',[sample(0,None,rss=5)])
    output=tmp_path/'report.md'
    p=run_cli(tmp_path,'--samples-before',before,'--samples-before',before,'--samples-after',after,'--output',output)
    assert p.returncode==0,p.stderr
    text=output.read_text()
    assert '## Process sample phases' in text
    assert '| Phase | Version | Samples | Runs | RSS median KiB | PSS+Swap median KiB | RSS peak KiB | PSS+Swap peak KiB | CPU seconds | CPU % | Observed processes | Incomplete memory samples |' in text
    assert '| work\\|x y | before | 2 | 1 | 20.00 | 60.00 | 30.00 | 60.00 | 2.00 | 200.00 | 1 | 0 |' in text
    assert '| (unmarked) | after | 1 | 1 | 5.00 | 60.00 | 5.00 | 60.00 | 0.00 | n/a | 1 | 0 |' in text
    p=run_cli(tmp_path,'--samples-after',after,'--output',output)
    assert p.returncode!=0
    assert output.read_text()==text


def test_cli_validation_failure_has_no_output(tmp_path):
    path=write(tmp_path/'a',[{'type':'broken'}]);out=tmp_path/'bad.md'
    assert run_cli(tmp_path,'--samples-before',path,'--output',out).returncode!=0
    assert not out.exists()


def test_cli_old_input_still_works(tmp_path):
    report=tmp_path/'native.json';report.write_text(json.dumps({'results':[{'status':'passed','messages':123,'cache':'first','median':{'rss_kib':2048},'measured_peak_kib':{'rss_kib':4096}}]}))
    out=tmp_path/'old.md'
    p=run_cli(tmp_path,'--native-before',report,'--output',out)
    assert p.returncode==0,p.stderr
    assert '123' in out.read_text() and '2.00' in out.read_text()
