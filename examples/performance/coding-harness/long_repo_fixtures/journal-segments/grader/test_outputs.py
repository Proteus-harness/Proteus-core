import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import pytest

ROOT = Path('/app')
CODE = ROOT / 'examples/performance/history-scale'
sys.path.insert(0, str(CODE))
import journal


def archiver():
    spec = importlib.util.spec_from_file_location('archive_under_test', CODE/'archive.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.archive


def source(path, n=7):
    path.mkdir()
    (path/'blobs').mkdir()
    raws, records = [], []
    for i in range(n):
        payload = {'text':'Юникод ' + str(i), 'array':[i, i+1]}
        value = {'storage':'inline', 'value':payload}
        if i % 2:
            data = json.dumps(payload, ensure_ascii=False).encode()
            sha = hashlib.sha256(data).hexdigest()
            (path/'blobs'/f'{sha}.json').write_bytes(data)
            value = {'storage':'blob', 'sha256':sha, 'bytes':len(data), 'relative_path':f'blobs/{sha}.json'}
        record = {'schema_version':21,'session_seq':i+17,'kind':'example','payload':value}
        raws.append(json.dumps(record, ensure_ascii=False, separators=(', ', ': ')).encode()+b'\n')
        records.append((record,payload))
    (path/'journal.jsonl').write_bytes(b''.join(raws))
    return raws, records


def fingerprint(path):
    return {str(p.relative_to(path)):p.read_bytes() for p in path.rglob('*') if p.is_file()}


def load_manifest(path):
    return json.loads((path/'journal-manifest.json').read_text())


def mutate_manifest(path, fn):
    m = load_manifest(path)
    fn(m)
    (path/'journal-manifest.json').write_text(json.dumps(m))


def test_roundtrip_preserves_bytes_blobs_and_reader_api(tmp_path):
    src, dst = tmp_path/'source', tmp_path/'archive'
    raws, expected = source(src)
    before = fingerprint(src)
    assert list(journal.records(src)) == expected
    archiver()(src, dst, segment_rows=3)
    assert fingerprint(src) == before
    assert list(journal.records(dst)) == expected
    manifest = load_manifest(dst)
    assert set(manifest) == {'version','segments'}
    assert manifest['version'] == 1
    assert len(manifest['segments']) == 3
    for index, entry in enumerate(manifest['segments']):
        data = b''.join(raws[index*3:(index+1)*3])
        assert entry == {'relative_path':f'segments/{index+1:06}.jsonl', 'sha256':hashlib.sha256(data).hexdigest(),
                         'bytes':len(data), 'first_seq':17+index*3, 'last_seq':min(23,19+index*3)}
        assert (dst/entry['relative_path']).read_bytes() == data
    assert fingerprint(src/'blobs') == fingerprint(dst/'blobs')


def test_cli_empty_and_existing_output(tmp_path):
    src = tmp_path/'empty'
    source(src,0)
    dst = tmp_path/'empty-archive'
    p = subprocess.run([sys.executable,str(CODE/'archive.py'),str(src),str(dst),'--segment-rows','2'],capture_output=True,text=True)
    assert p.returncode == 0, p.stderr
    assert load_manifest(dst) == {'version':1,'segments':[]}
    assert list(journal.records(dst)) == []
    marker = dst/'marker';marker.write_text('preserve')
    with pytest.raises((ValueError,FileExistsError)):
        archiver()(src,dst)
    assert marker.read_text() == 'preserve'


@pytest.mark.parametrize('limit',[0,-1,True,1.5,'2',None])
def test_invalid_row_limits_leave_no_output(tmp_path,limit):
    src,dst=tmp_path/'source',tmp_path/'out';source(src)
    with pytest.raises((ValueError,TypeError)):
        archiver()(src,dst,segment_rows=limit)
    assert not dst.exists()


@pytest.mark.parametrize('damage',['sequence','blob','truncated','blank','storage','schema'])
def test_invalid_legacy_cannot_publish_partial_archive(tmp_path,damage):
    src,dst=tmp_path/'source',tmp_path/'out';raws,_=source(src)
    if damage=='sequence':
        row=json.loads(raws[-1]);row['session_seq']+=5;raws[-1]=json.dumps(row).encode()+b'\n'
    if damage=='blob':next((src/'blobs').iterdir()).write_bytes(b'wrong')
    if damage=='truncated':raws[-1]=raws[-1].rstrip(b'\n')
    if damage=='blank':raws.insert(3,b'\n')
    if damage in ('schema','storage'):
        row=json.loads(raws[-1])
        if damage=='schema':row['schema_version']=20
        else:row['payload']['storage']='weird'
        raws[-1]=json.dumps(row).encode()+b'\n'
    (src/'journal.jsonl').write_bytes(b''.join(raws))
    with pytest.raises(ValueError):list(journal.records(src))
    with pytest.raises(ValueError):archiver()(src,dst,segment_rows=2)
    assert not dst.exists()
    assert not list(tmp_path.glob('.journal-archive-*'))


@pytest.mark.parametrize('field,value',[('version',True),('version',2),('version','1'),('segments',{}),('extra',1)])
def test_manifest_types_strict(tmp_path,field,value):
    src,dst=tmp_path/'source',tmp_path/'out';source(src);archiver()(src,dst)
    mutate_manifest(dst,lambda m:m.__setitem__(field,value))
    with pytest.raises(ValueError):list(journal.records(dst))


@pytest.mark.parametrize('field,value',[('relative_path','../escape'),('relative_path','/tmp/file'),('relative_path','segments/000003.jsonl'),
    ('bytes',True),('bytes',-1),('bytes',3),('first_seq',True),('first_seq',99),('last_seq',999),('sha256','a'*64),('extra',1)])
def test_segment_metadata_strict(tmp_path,field,value):
    src,dst=tmp_path/'source',tmp_path/'out';source(src);archiver()(src,dst,segment_rows=3)
    mutate_manifest(dst,lambda m:m['segments'][0].__setitem__(field,value))
    with pytest.raises(ValueError):list(journal.records(dst))


def test_segment_validated_before_first_record_yield(tmp_path):
    src,dst=tmp_path/'source',tmp_path/'out';source(src);archiver()(src,dst,segment_rows=3)
    m=load_manifest(dst);entry=m['segments'][0];file=dst/entry['relative_path']
    rows=file.read_bytes().splitlines(keepends=True)
    row=json.loads(rows[-1]);row['session_seq']=999;rows[-1]=json.dumps(row).encode()+b'\n'
    data=b''.join(rows);file.write_bytes(data);entry['bytes']=len(data);entry['sha256']=hashlib.sha256(data).hexdigest();entry['last_seq']=999
    (dst/'journal-manifest.json').write_text(json.dumps(m))
    with pytest.raises(ValueError):next(iter(journal.records(dst)))


@pytest.mark.parametrize('which',['segment','segment-dir','blob','blob-dir'])
def test_symlinks_rejected_even_for_in_tree_targets(tmp_path,which):
    src,dst=tmp_path/'source',tmp_path/'out';source(src);archiver()(src,dst,segment_rows=3)
    if which=='segment':path=dst/'segments/000001.jsonl'
    elif which=='blob':path=next((dst/'blobs').iterdir())
    else:path=dst/('segments' if which=='segment-dir' else 'blobs')
    moved=path.with_name(path.name+'.real');path.rename(moved);path.symlink_to(moved.name,target_is_directory=moved.is_dir())
    with pytest.raises(ValueError):list(journal.records(dst))


def test_ambiguous_and_missing_formats(tmp_path):
    src,dst=tmp_path/'source',tmp_path/'out';source(src);archiver()(src,dst)
    (dst/'journal.jsonl').write_text('')
    with pytest.raises(ValueError):list(journal.records(dst))
    empty=tmp_path/'missing';empty.mkdir()
    with pytest.raises(ValueError):list(journal.records(empty))


def test_existing_consumers_and_public_helpers(tmp_path):
    src,dst=tmp_path/'source',tmp_path/'out'
    src.mkdir();(src/'journal.jsonl').write_text('')
    archiver()(src,dst)
    spec=importlib.util.spec_from_file_location('usage_under_test',ROOT/'examples/performance/code-mode/measurements.py')
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    assert module.measure(dst)['all_tool_calls'] == 0
    m={'id':'m','role':'User','parts':[{'payload':{'Text':{'text':'hello'}}}]}
    assert journal.text(m)=='hello'
    assert journal.message_digest([m])[0]==1
