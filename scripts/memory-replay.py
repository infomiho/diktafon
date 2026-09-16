#!/usr/bin/env python3
"""Replay a saved WAV in one client process and summarize macOS memory snapshots."""
import argparse
import csv
import os
from pathlib import Path
import re
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('wav', type=Path)
parser.add_argument('--repeat', type=int, default=20)
parser.add_argument('--binary', type=Path, default=Path('target/release/diktafon'))
args = parser.parse_args()
if args.repeat < 1:
    parser.error('--repeat must be positive')
output = Path(tempfile.mkdtemp(prefix='diktafon-memory-'))
env = dict(os.environ, DIKTAFOND_SOCKET=str(output / 'daemon.sock'),
           DIKTAFOND_NO_HISTORY='1', DIKTAFOND_IDLE_SECS='10')
env.pop('MallocStackLogging', None)
print(f'Results: {output}', flush=True)
with (output / 'run.log').open('w') as log:
    subprocess.run([str(args.binary.resolve()), '--transcribe-file', str(args.wav.resolve()),
                    '--repeat', str(args.repeat), '--memory-dir', str(output)],
                   env=env, stdout=log, stderr=log, check=True,
                   timeout=max(300, args.repeat * 120))
with (output / 'summary.csv').open('w') as summary:
    writer = csv.writer(summary)
    writer.writerow(['cycle', 'footprint', 'live_heap', 'unused_allocator'])
    for cycle in range(args.repeat + 1):
        footprint = (output / f'{cycle:03}-footprint.txt').read_text()
        vmmap = (output / f'{cycle:03}-vmmap.txt').read_text()
        total = [line.split() for line in vmmap.splitlines() if line.startswith('TOTAL')][-1]
        size = re.search(r'Footprint:\s*([^\n(]+)', footprint).group(1).strip()
        row = [cycle, size, total[6], total[7]]
        writer.writerow(row)
        print(', '.join(map(str, row)))
