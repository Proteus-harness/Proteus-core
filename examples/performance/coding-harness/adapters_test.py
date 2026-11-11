"""Offline checks for credential selection and disposable adapter routing."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import open_code_native

from open_code_native import active_export, prepare_auth_export, write_private_json


class AuthIsolation(unittest.TestCase):
    def test_export_stages_one_active_provider_without_other_credentials(self):
        records = [
            {'id': 'other', 'integrationID': 'unrelated', 'active': True, 'value': {'key': 'private-other'}},
            {'id': 'old', 'integrationID': 'opencode-go', 'active': False, 'value': {'key': 'private-old'}},
            {'id': 'selected', 'integrationID': 'opencode-go', 'active': True,
             'value': {'type': 'key', 'key': 'synthetic-selected'}},
        ]
        with tempfile.TemporaryDirectory() as directory:
            source, destination = Path(directory)/'source.json', Path(directory)/'selected.json'
            source.write_text(json.dumps(records))
            prepare_auth_export(source, destination, 'opencode-go')
            self.assertEqual(json.loads(destination.read_text()), [records[-1]])
            self.assertEqual(destination.stat().st_mode & 0o777, 0o600)

    def test_absent_and_ambiguous_active_credentials_fail_closed(self):
        active = {'integrationID': 'opencode-go', 'active': True}
        for records in [[], [{**active, 'active': False}], [active, active]]:
            with self.subTest(records=records), self.assertRaises(ValueError):
                active_export(records, 'opencode-go')

    def test_auth_write_cannot_replace_an_existing_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'auth.json'
            write_private_json(path, {'synthetic': 'first'})
            with self.assertRaises(FileExistsError):
                write_private_json(path, {'synthetic': 'second'})
            self.assertEqual(json.loads(path.read_text()), {'synthetic': 'first'})


class FlashAuxiliaryGate(unittest.TestCase):
    def metadata(self, title):
        wanted = {'providerID': 'opencode-go', 'model': 'deepseek-v4.1-flash', 'variant': 'high'}
        return {
            'model.list': {'data': [{'id': wanted['model'], 'providerID': wanted['providerID'], 'variants': []}]},
            'plugin.list': [],
            'config.get': [{'info': {'model': wanted, 'agents': {'title': {'model': title}}}}],
        }

    def test_missing_or_different_title_cannot_generate(self):
        for title in [None, {'providerID':'opencode-go','model':'other-model','variant':'high'}]:
            with self.subTest(title=title), tempfile.TemporaryDirectory() as directory:
                metadata = self.metadata(title)
                with patch.object(open_code_native, 'api_json', side_effect=lambda b,e,u,op,**kw:metadata[op]):
                    with self.assertRaisesRegex(RuntimeError, 'refusing generation'):
                        open_code_native.save_registry('binary', {}, 'http://unused',
                            'opencode-go/deepseek-v4.1-flash#high', Path(directory))

    def test_exact_main_and_title_binding_is_retained_as_proof(self):
        metadata = self.metadata({'providerID':'opencode-go','model':'deepseek-v4.1-flash','variant':'high'})
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(open_code_native, 'api_json', side_effect=lambda b,e,u,op,**kw:metadata[op]):
                open_code_native.save_registry('binary', {}, 'http://unused',
                    'opencode-go/deepseek-v4.1-flash#high', Path(directory))
            proof = json.loads((Path(directory)/'registry.json').read_text())
            self.assertTrue(proof['auxiliary_model_verified'])
            self.assertEqual(proof['model_bindings']['main'], proof['model_bindings']['title'])


if __name__ == '__main__':
    unittest.main()
