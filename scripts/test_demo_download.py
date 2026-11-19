import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class DemoDownloadTest(unittest.TestCase):
    def test_each_downloaded_or_cached_shard_advances_progress(self):
        for cached, failed in [(False, False), (True, False), (False, True)]:
            with self.subTest(cached=cached, failed=failed), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                data = root / 'data' / 'base_data_climbmix'
                data.mkdir(parents=True)
                if cached:
                    for index in [0, 1, 6542]:
                        (data / f'shard_{index:05d}.parquet').write_bytes(b'parquet')
                package = root / 'nanochat'
                package.mkdir()
                (package / '__init__.py').write_text('__path__ = ' + repr([
                    str(ROOT / 'demo/nanochat/experiment/nanochat'),
                    str(ROOT / 'demo/nanochat/base/nanochat'),
                ]))
                (root / 'sitecustomize.py').write_text('''
import os
import sys
import types
import time
time.sleep = lambda _: None
torch = types.ModuleType('torch')
torch.bfloat16 = torch.float16 = torch.float32 = object()
torch.cuda = types.SimpleNamespace(is_available=lambda: False)
sys.modules['torch'] = torch
sys.modules['torch.distributed'] = types.ModuleType('torch.distributed')
filelock = types.ModuleType('filelock')
filelock.FileLock = object
sys.modules['filelock'] = filelock
arrow = types.ModuleType('pyarrow')
arrow.parquet = types.ModuleType('pyarrow.parquet')
sys.modules['pyarrow'] = arrow
sys.modules['pyarrow.parquet'] = arrow.parquet
requests = types.ModuleType('requests')
class Response:
    def raise_for_status(self): pass
    def iter_content(self, chunk_size): return [b'parquet']
def get(url, **kwargs):
    if os.environ['FAIL_DOWNLOAD'] == '1' and 'shard_00001' in url:
        raise RuntimeError('connection interrupted')
    return Response()
requests.get = get
requests.RequestException = RuntimeError
sys.modules['requests'] = requests
''')
                result = subprocess.run(
                    [sys.executable, '-m', 'nanochat.dataset', '-n', '2', '-w', '2'],
                    cwd=root,
                    env={**os.environ, 'PYTHONPATH': str(root),
                         'NANOCHAT_BASE_DIR': str(data.parent), 'FAIL_DOWNLOAD': str(int(failed)),
                         'PYTHONDONTWRITEBYTECODE': '1'},
                    text=True, capture_output=True, timeout=30,
                )
                self.assertEqual(result.returncode, int(failed), result.stderr)
                reports = [json.loads(line.removeprefix('::orx:: '))
                           for line in result.stdout.splitlines() if line.startswith('::orx:: ')]
                self.assertEqual([report['step'] for report in reports], list(range(3 if failed else 4)))
                self.assertTrue(all(report['of'] == 3 for report in reports))
                self.assertEqual(len(list(data.glob('*.parquet'))), 2 if failed else 3)
                self.assertEqual(list(data.glob('*.tmp')), [])


if __name__ == '__main__':
    unittest.main()
