"""Controls for hidden fixtures, test activation and false successful exits."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import polyglot_corpus as corpus
import polyglot_grader as grader


class PolyglotCorpusTests(unittest.TestCase):
    def setUp(self):
        corpus.DATA.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix='corpus-test-', dir=corpus.DATA)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def fixture(self, language='python'):
        source = self.root/'source'/'sample'
        (source/'.meta').mkdir(parents=True)
        (source/'.docs').mkdir()
        (source/'.docs/instructions.md').write_text('Implement the answer.\n')
        if language == 'python':
            names = {'solution': ['answer.py'], 'test': ['answer_test.py'],
                     'example': ['.meta/example.py']}
            (source/'answer.py').write_text('value = 0\n')
            (source/'answer_test.py').write_text('import answer\ndef test_answer():\n    assert answer.value == 42\n')
            (source/'.meta/example.py').write_text('value = 42\n')
        else:
            names = {'solution': ['answer.go'], 'test': ['answer_test.go'],
                     'example': ['.meta/example.go'], 'editor': ['cases_test.go']}
            for name in ('answer.go', 'answer_test.go', 'cases_test.go', '.meta/example.go'):
                (source/name).write_text('package sample\n')
            (source/'go.mod').write_text('module sample\n\ngo 1.18\n')
        (source/'.meta/config.json').write_text(json.dumps({'files': names}))
        cases = self.root/'cases'
        with patch.object(corpus, 'source_cases', return_value=[(language, source)]):
            corpus.prepare(cases, self.root/'source')
        return cases/(language+'--sample')

    def test_go_editor_tests_hidden_and_module_configuration_preserved(self):
        case = self.fixture('go')
        self.assertFalse((case/'repo/cases_test.go').exists())
        self.assertFalse((case/'repo/answer_test.go').exists())
        self.assertFalse((case/'repo/.meta').exists())
        self.assertTrue((case/'repo/go.mod').is_file())
        self.assertTrue((case/'grader/pristine/cases_test.go').is_file())
        self.assertIn('go.mod', json.loads((case/'case.json').read_text())['immutable_files'])

    def test_skip_activation_preserves_explicit_optional_tests(self):
        source = "xtest('a',()=>{});\nxit('b',()=>{});\ntest.skip('optional',()=>{});\n"
        activated = corpus.activated_test('javascript', source)
        self.assertIn("test('a'", activated)
        self.assertIn("it('b'", activated)
        self.assertIn("test.skip('optional'", activated)
        self.assertNotIn('xit(', activated)

    def test_authoritative_tests_override_candidate_injection(self):
        if not (grader.PYTHON/'bin/python').exists():
            self.skipTest('Prepared Python grader required')
        case = self.fixture()
        candidate = case/'repo'
        (candidate/'answer_test.py').write_text('def test_fake():\n    assert True\n')
        bad = corpus.grade(case, candidate, timeout=20)
        self.assertFalse(bad['passed'])
        self.assertIn('assert answer.value == 42', bad['output'])
        (candidate/'answer.py').write_text('value = 42\n')
        good = corpus.grade(case, candidate, timeout=20)
        self.assertTrue(good['passed'])
        self.assertEqual(good['executed_tests'], 1)

    def test_successful_exit_without_running_tests_is_not_a_pass(self):
        if not (grader.PYTHON/'bin/python').exists():
            self.skipTest('Prepared Python grader required')
        case = self.fixture()
        (case/'repo/answer.py').write_text('import os\nos._exit(0)\n')
        result = corpus.grade(case, case/'repo', timeout=20)
        self.assertEqual(result['returncode'], 0)
        self.assertEqual(result['executed_tests'], 0)
        self.assertFalse(result['passed'])

    def test_linked_solution_is_rejected(self):
        case = self.fixture()
        target = case/'repo/answer.py'
        target.unlink()
        (case/'repo/answer-real.py').write_text('value = 42\n')
        target.symlink_to('answer-real.py')
        with patch.object(grader, 'execute', return_value={'returncode': 0, 'executed_tests': 1}):
            result = corpus.grade(case, case/'repo')
        self.assertFalse(result['passed'])
        self.assertEqual(result['missing_solution_files'], ['answer.py'])

    def test_empty_go_and_skipped_java_do_not_count_as_execution(self):
        self.assertEqual(grader.executed_tests('go', '{broken}\n{"Action":"pass","Package":"x"}', self.root), 0)
        xml = self.root/'build/test-results/test'
        xml.mkdir(parents=True)
        (xml/'sample.xml').write_text('<testsuite tests="3" skipped="3"/>')
        self.assertEqual(grader.executed_tests('java', '', self.root), 0)

    def test_agent_caches_are_workspace_writable_and_dependencies_readonly(self):
        runtime = self.root/'runtime'
        runtime.mkdir()
        mounts, environment = grader.agent_runtime(runtime, 'rust')
        cache = self.root/'workspace/.benchmark-cache'
        for key, name in [('CARGO_HOME', 'cargo'), ('GRADLE_USER_HOME', 'gradle'),
                          ('GOCACHE', 'go-build'), ('NPM_CONFIG_CACHE', 'npm')]:
            self.assertEqual(environment[key], '/workspace/.benchmark-cache/' + name)
            self.assertTrue((cache/name).is_dir())
        self.assertNotIn('HOME', environment)
        self.assertEqual((cache/'cargo/registry').readlink(), Path('/polyglot/cargo-home/registry'))
        self.assertIn((grader.TOOLS/'cargo-home', '/polyglot/cargo-home'), mounts)
        mounts, _ = grader.agent_runtime(runtime, 'javascript')
        self.assertIn((grader.TOOLS/'npm/node_modules', '/workspace/node_modules'), mounts)

    def test_known_preflight_defects_do_not_become_successful_solutions(self):
        result = {'starter': {'passed': True}, 'reference': {'passed': True}}
        result.update(corpus.control_disposition('go--ledger', result))
        self.assertTrue(result['starter_pass_allowed'])
        self.assertTrue(corpus.control_valid(result))
        result = {'starter': {'passed': False}, 'reference': {'passed': False}}
        result.update(corpus.control_disposition('go--counter', result))
        self.assertFalse(result['evaluable'])
        self.assertEqual(result['reason'], 'upstream_no_tests')


if __name__ == '__main__':
    unittest.main()
