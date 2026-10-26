import importlib.util
import json
import pathlib
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

SOURCE=pathlib.Path(__file__).resolve().parents[1]/'src/integration/assets/native-context-service.py'
spec=importlib.util.spec_from_file_location('native_context_service',SOURCE)
service=importlib.util.module_from_spec(spec)
spec.loader.exec_module(service)

class NativeContextServiceTest(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory()
        self.root=pathlib.Path(self.temp.name)
        self.lease=self.root/'lease.json'
        self.config={'token':'test-capability','context':{'agent':'claude','process_id':123,'session_id':'exact-session','session_path':str(self.root/'source.jsonl')}}
        self.lease.write_text(json.dumps(self.config));self.lease.chmod(0o600)
        self.server=service.ContextServer(self.lease)
    def tearDown(self):
        self.temp.cleanup()
    def test_lease_must_be_private(self):
        self.lease.chmod(0o644)
        with self.assertRaisesRegex(RuntimeError,'not private'):service.read_private(self.lease)
    def test_wrong_capability_never_opens_or_closes_native_editor(self):
        with patch.object(service,'app_control') as control:
            with self.assertRaisesRegex(RuntimeError,'context_not_authorized'):
                self.server.request({'action':'close','token':'wrong'})
            control.assert_not_called()
    def test_nonempty_draft_is_preserved(self):
        draft=self.root/'draft';draft.write_text('UNSENT CONTENT')
        self.server.armed=True
        with patch.object(service,'owns_editor_process',return_value=True):
            result=self.server.handle_editor(self.config,{'buffer_path':str(draft),'parent_pid':123,'editor_pid':456})
        self.assertFalse(result['ok']);self.assertTrue(result['preserve'])
        self.assertEqual(draft.read_text(),'UNSENT CONTENT')
        self.assertIn('source_has_draft',self.server.editor_error)
    def test_close_is_exactly_once_even_if_the_first_ack_is_lost(self):
        snapshot={'session_id':'exact-session','complete':'history'}
        digest=service.hashlib.sha256(json.dumps(snapshot,sort_keys=True,separators=(',',':')).encode()).hexdigest()
        self.server.snapshot_id=digest
        with patch.object(self.server,'monitor_source'),patch.object(self.server,'capture',return_value=snapshot),patch.object(self.server,'prove_native_queue_empty'),patch.object(self.server,'release_and_wait_frontend'),patch.object(service,'app_control',side_effect=RuntimeError('ack lost')) as control:
            command={'token':'test-capability','action':'close','snapshot_id':digest}
            with self.assertRaisesRegex(RuntimeError,'ack lost'):self.server.request(command)
            with self.assertRaisesRegex(RuntimeError,'never repeated'):self.server.request(command)
            self.assertEqual(control.call_count,1)
    def test_late_context_change_after_editor_release_never_exits(self):
        old={'state':'old'};new={'state':'changed'}
        digest=service.hashlib.sha256(json.dumps(old,sort_keys=True,separators=(',',':')).encode()).hexdigest()
        self.server.snapshot_id=digest
        with patch.object(self.server,'monitor_source'),patch.object(self.server,'capture',side_effect=[old,new]),patch.object(self.server,'prove_native_queue_empty'),patch.object(self.server,'release_and_wait_frontend'),patch.object(service,'app_control') as control:
            with self.assertRaisesRegex(RuntimeError,'changed during frontend release'):
                self.server.request({'token':'test-capability','action':'close','snapshot_id':digest})
            control.assert_not_called()

    def test_changed_snapshot_never_closes_source(self):
        self.server.snapshot_id='old'
        with patch.object(self.server,'monitor_source'),patch.object(self.server,'capture',return_value={'edited':'history'}),patch.object(self.server,'prove_native_queue_empty'),patch.object(service,'app_control') as control:
            with self.assertRaisesRegex(RuntimeError,'source_changed'):
                self.server.request({'token':'test-capability','action':'close','snapshot_id':'old'})
            control.assert_not_called()
    def test_native_codex_queue_is_checked_for_the_exact_session(self):
        home=self.root/'codex';directory=home/'sessions/2026/10/26';directory.mkdir(parents=True)
        database=home/'queue_1.sqlite'
        connection=sqlite3.connect(database)
        connection.execute('CREATE TABLE queued_items(thread_id TEXT)')
        connection.execute('INSERT INTO queued_items VALUES (?)',('unrelated-sibling',));connection.commit()
        config={'context':{'agent':'codex','session_id':'exact-session','session_path':str(directory/'rollout.jsonl')}}
        self.server.prove_native_queue_empty(config)
        connection.execute('INSERT INTO queued_items VALUES (?)',('exact-session',));connection.commit()
        with self.assertRaisesRegex(RuntimeError,'queued messages'):self.server.prove_native_queue_empty(config)
        connection.close()
    def test_one_native_dequeue_cannot_hide_a_second_queued_prompt(self):
        source=self.root/'source.jsonl'
        rows=[{'type':'queue-operation','operation':op} for op in ['enqueue','enqueue','dequeue']]
        source.write_text(''.join(json.dumps(row)+'\n' for row in rows))
        with self.assertRaisesRegex(RuntimeError,'queued messages'):self.server.prove_native_queue_empty(self.config)
        rows.append({'type':'queue-operation','operation':'dequeue'})
        source.write_text(''.join(json.dumps(row)+'\n' for row in rows))
        self.server.prove_native_queue_empty(self.config)
    def test_unknown_native_queue_operation_fails_closed(self):
        source=self.root/'source.jsonl';source.write_text(json.dumps({'type':'queue-operation','operation':'new-format'})+'\n')
        with self.assertRaisesRegex(RuntimeError,'unknown native queue operation'):self.server.prove_native_queue_empty(self.config)
    def test_editor_argument_contract_allows_paths_with_spaces(self):
        args=service.parse_arguments(['editor','--lease','/private/example lease.json','/private/draft file.md'])
        self.assertEqual(args.buffer,'/private/draft file.md')

if __name__=='__main__':unittest.main()
