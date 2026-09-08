import importlib.util
import json
import os
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("dreamer_rollout",ROOT/"scripts/dreamer-railway-rollout.py")
rollout=importlib.util.module_from_spec(spec);spec.loader.exec_module(rollout)


class FakeChild:
    def __init__(self,command,**kwargs):
        self.fd=os.dup(kwargs["pass_fds"][0]);self.returncode=None
    def wait(self,timeout=None):
        if self.fd>=0:os.close(self.fd);self.fd=-1
        self.returncode=1;return 1
    def poll(self):return self.returncode
    def terminate(self):self.wait()
    def kill(self):self.wait()


class RailwayRolloutTests(unittest.TestCase):
    def test_staging_keeps_value_out_of_argv_and_skips_deployment(self):
        with patch.object(rollout,"railway",return_value=b"") as railway, patch.object(rollout,"variables",return_value={"DREAMER_MODEL_TOKEN":"secret-value"}):
            rollout.stage_variable(SimpleNamespace(),"dreamer","DREAMER_MODEL_TOKEN","secret-value")
            argv=railway.call_args.args[1]
            self.assertIn("--stdin",argv);self.assertIn("--skip-deploys",argv)
            self.assertNotIn("secret-value",argv)
            self.assertEqual(railway.call_args.kwargs["payload"],b"secret-value")

    def test_deployed_gate_uses_prefixed_http_identity(self):
        owner=SimpleNamespace(request=Mock(side_effect=[{"user":{"id":rollout.OWNER_REF},"capabilities":["admin","credential:manage"]},{"build_revision":"a"*40},{"status":"ready"}]))
        rollout.deployed_gate(owner,"a"*40)
        self.assertIn("user:"+rollout.OWNER,rollout.PRIVATE_NODE)

    def bridge(self,directory,report_text,child=FakeChild):
        path=Path(directory)/"canary.json";path.write_text(report_text)
        args=SimpleNamespace(expected_revision="a"*40,summary_preference="disabled_then_enabled",flag_wait_seconds=30,canary_report=path)
        owner=SimpleNamespace(base="https://brunn.ai/api",request=Mock(return_value={}))
        response={"user":{"id":"user:11111111-1111-4111-8111-111111111111","external_ref":"brunn-dreamer-canary:22222222-2222-4222-8222-222222222222"},"credential":{"token":"fixture-private"}}
        report={"calls":[]}
        return args,owner,response,report

    def test_truncated_report_still_triggers_scoped_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            args,owner,response,report=self.bridge(directory,'{"unfinished":')
            with patch.object(rollout,"deployed_gate",return_value={"feature_flags":{"dreamer_summary_reads_enabled":False}}),patch.object(rollout,"private_request",return_value=response),patch.object(rollout.subprocess,"Popen",FakeChild),patch.object(rollout.CANARY,"cleanup",return_value={"canonical_purge_verified":True}) as cleanup:
                rollout.execute_canary(args,owner,report,lambda:None)
                self.assertEqual(cleanup.call_args.args[2],rollout.OWNER_REF)
                self.assertEqual(report["bridge_cleanup"]["canonical_purge_verified"],True)
                self.assertNotIn("fixture-private",json.dumps(report))

    def test_child_launch_failure_still_cleans_provisioned_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            args,owner,response,report=self.bridge(directory,"")
            with patch.object(rollout,"deployed_gate",return_value={"feature_flags":{"dreamer_summary_reads_enabled":False}}),patch.object(rollout,"private_request",return_value=response),patch.object(rollout.subprocess,"Popen",side_effect=OSError("launch failed")),patch.object(rollout.CANARY,"cleanup",return_value={"canonical_purge_verified":True}) as cleanup:
                with self.assertRaises(OSError):rollout.execute_canary(args,owner,report,lambda:None)
                cleanup.assert_called_once()

    def test_broken_pipe_does_not_skip_cleanup_with_double_close(self):
        with tempfile.TemporaryDirectory() as directory:
            args,owner,response,report=self.bridge(directory,"")
            finished=SimpleNamespace(wait=lambda:1,poll=lambda:1)
            with patch.object(rollout,"deployed_gate",return_value={"feature_flags":{"dreamer_summary_reads_enabled":False}}),patch.object(rollout,"private_request",return_value=response),patch.object(rollout.subprocess,"Popen",return_value=finished),patch.object(rollout.CANARY,"cleanup",return_value={"canonical_purge_verified":True}) as cleanup:
                with self.assertRaises(BrokenPipeError):rollout.execute_canary(args,owner,report,lambda:None)
                cleanup.assert_called_once()

    def test_completed_canary_cleanup_is_not_repeated(self):
        with tempfile.TemporaryDirectory() as directory:
            args,owner,response,report=self.bridge(directory,json.dumps({"status":"passed","cleanup":{"canonical_purge_verified":True}}))
            with patch.object(rollout,"deployed_gate",return_value={"feature_flags":{"dreamer_summary_reads_enabled":False}}),patch.object(rollout,"private_request",return_value=response),patch.object(rollout.subprocess,"Popen",FakeChild),patch.object(rollout.CANARY,"cleanup") as cleanup:
                rollout.execute_canary(args,owner,report,lambda:None)
                cleanup.assert_not_called();self.assertEqual(report["status"],"canary_complete")


if __name__=="__main__":unittest.main()
