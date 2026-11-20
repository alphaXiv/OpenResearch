"""Run with the nanochat demo's CPU environment: python scripts/test_demo_tail.py."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class DemoTailTest(unittest.TestCase):
    def test_validation_keeps_training_coordinates_and_finalization_counts_work(self):
        import pyarrow as pa
        import pyarrow.parquet as pq
        import torch

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repo = root / 'repo'
            shutil.copytree(ROOT / 'demo/nanochat/base', repo)
            shutil.copytree(ROOT / 'demo/nanochat/experiment', repo, dirs_exist_ok=True)
            shutil.copyfile(ROOT / 'agent-skills/orx-compute/assets/orx_run.py', repo / 'orx_run.py')
            data = root / 'data'
            env = {**os.environ, 'NANOCHAT_BASE_DIR': str(data), 'OMP_NUM_THREADS': '1',
                   'MKL_NUM_THREADS': '1', 'TORCHDYNAMO_DISABLE': '1', 'PYTHONUNBUFFERED': '1', 'PYTHONIOENCODING': 'utf-8'}
            sys.path.insert(0, str(repo))
            from nanochat.tokenizer import RustBPETokenizer
            text = (repo / 'README.md').read_text(encoding='utf-8')
            tokenizer = RustBPETokenizer.train_from_iterator([text], 512)
            tokenizer.save(str(data / 'tokenizer'))
            special = {tokenizer.encode_special(token) for token in tokenizer.get_special_tokens()}
            sizes = [0 if token in special else len(tokenizer.decode_single_token_bytes(token))
                     for token in range(tokenizer.get_vocab_size())]
            torch.save(torch.tensor(sizes, dtype=torch.int32), data / 'tokenizer/token_bytes.pt')
            shards = data / 'base_data_climbmix'
            shards.mkdir()
            for name in ['shard_00000.parquet', 'shard_06542.parquet']:
                pq.write_table(pa.table({'text': [text] * 64}), shards / name)
            conversation = {'messages': [{'role': 'user', 'content': 'Hi'},
                                          {'role': 'assistant', 'content': 'Hello'}]}
            fixtures = [
                ('HuggingFaceTB/smol-smoltalk', 'default', ['train', 'test'], conversation),
                ('cais/mmlu', 'all', ['test'], {'question': 'One?', 'choices': ['1', '2', '3', '4'],
                                               'answer': 0, 'subject': 'abstract_algebra'}),
                ('openai/gsm8k', 'main', ['test'], {'question': 'One?', 'answer': '#### 1'}),
            ]
            for name, subset, splits, row in fixtures:
                for split in splits:
                    cache = data / 'task_data' / name.replace('/', '--') / subset / split
                    cache.mkdir(parents=True)
                    pq.write_table(pa.Table.from_pylist([row] * 6000), cache / '00000.parquet')
                    (cache / 'manifest.json').write_text(json.dumps(['00000.parquet']))
            commands = [
                ('scripts.base_train', ['--depth=1', '--head-dim=64', '--window-pattern=L',
                 '--max-seq-len=64', '--device-batch-size=1', '--total-batch-size=64',
                 '--num-iterations=2', '--eval-every=1', '--eval-tokens=64',
                 '--core-metric-every=-1', '--sample-every=1'], 'train-finalize', 9),
                ('scripts.chat_sft', ['--model-tag=d1', '--num-iterations=3', '--load-optimizer=0',
                 '--mmlu-epochs=0', '--gsm8k-epochs=0', '--eval-every=1', '--eval-tokens=64',
                 '--chatcore-every=-1'], 'sft-finalize', 2),
            ]
            outputs = []
            for module, args, phase, total in commands:
                result = subprocess.run([sys.executable, '-m', module, '--device-type=cpu', *args],
                                        cwd=repo, env=env, capture_output=True, text=True, encoding='utf-8', timeout=180)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                outputs.append((phase, total, result.stdout))
            for phase, total, output in outputs:
                with self.subTest(phase=phase):
                    records = [json.loads(line.removeprefix('::orx:: '))
                               for line in output.splitlines() if line.startswith('::orx:: ')]
                    starts = [record for record in records if record.get('phase') == phase]
                    self.assertEqual(len(starts), 1, output)
                    self.assertEqual(starts[0]['step'], 0)
                    self.assertEqual(starts[0]['of'], total)
                    start = records.index(starts[0])
                    metric = 'val_bpb' if phase == 'train-finalize' else 'sft_val_bpb'
                    self.assertTrue(any(record.get('step') == 2 and metric in record.get('m', {})
                                        for record in records[:start]))
                    self.assertTrue(all(metric not in record.get('m', {}) for record in records[start:]))
                    self.assertEqual([record['step'] for record in records[start:]], list(range(total + 1)))
            self.assertTrue(list((data / 'base_checkpoints/d1').glob('model_*.pt')))
            self.assertTrue(list((data / 'chatsft_checkpoints/d1').glob('model_*.pt')))


if __name__ == '__main__':
    unittest.main()
