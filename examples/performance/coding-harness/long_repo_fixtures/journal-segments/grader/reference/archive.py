"""Create a verified segmented journal archive without mutating a source."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
from journal import raw_records


def archive(session, output, segment_rows=1000):
    session, output = Path(session).resolve(), Path(output).absolute()
    if type(segment_rows) is not int or segment_rows <= 0:
        raise ValueError('segment_rows must be a positive integer')
    if output.exists() or output.is_symlink():
        raise ValueError('output exists')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.journal-archive-', dir=output.parent) as temporary:
        stage = Path(temporary) / 'output'
        (stage / 'segments').mkdir(parents=True)
        (stage / 'blobs').mkdir()
        segments = []
        file = None
        try:
            rows = 0
            for record, _, raw in raw_records(session):
                if rows == 0:
                    relative = f'segments/{len(segments)+1:06}.jsonl'
                    file = (stage / relative).open('wb')
                    sha, size, first = hashlib.sha256(), 0, record['session_seq']
                file.write(raw)
                sha.update(raw)
                size += len(raw)
                last = record['session_seq']
                rows += 1
                stored = record['payload']
                if stored['storage'] == 'blob':
                    destination = stage / stored['relative_path']
                    if not destination.exists():
                        shutil.copyfile(session / stored['relative_path'], destination)
                if rows == segment_rows:
                    file.close()
                    file = None
                    segments.append(dict(relative_path=relative, sha256=sha.hexdigest(), bytes=size, first_seq=first, last_seq=last))
                    rows = 0
            if rows:
                file.close()
                file = None
                segments.append(dict(relative_path=relative, sha256=sha.hexdigest(), bytes=size, first_seq=first, last_seq=last))
            (stage / 'journal-manifest.json').write_text(json.dumps(dict(version=1, segments=segments)) + '\n')
            if output.exists() or output.is_symlink():
                raise ValueError('output exists')
            stage.rename(output)
        finally:
            if file:
                file.close()
    return output


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('session', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--segment-rows', type=int, default=1000)
    a = p.parse_args()
    archive(a.session, a.output, a.segment_rows)


if __name__ == '__main__':
    main()
