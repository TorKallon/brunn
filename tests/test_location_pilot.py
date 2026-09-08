import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from scripts.location_pilot import freeze, load_bundle, model_environment, validate_packet, validate_plan, verify_subscription


def packet():
    return {"schema":"location.evidence.v1", "interval":{"from":"2026-09-06T07:00:00Z","to":"2026-09-07T07:00:00Z","timezone":"America/Los_Angeles"}, "completeness":{"complete":True},"fingerprint_complete":True,"evidence_fingerprint":"sha256:synthetic","reports":[]}


class LocationPilotTests(unittest.TestCase):
    def test_freeze_detects_question_or_evidence_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.json"
            source.write_text(json.dumps(packet()))
            freeze(source, root / "bundle", "2026-09-08T00:00:00Z")
            load_bundle(root / "bundle")
            self.assertEqual((root / "bundle/packet.json").stat().st_mode & 0o777, 0o600)
            (root / "bundle/questions.json").write_text('{"questions":[]}')
            with self.assertRaisesRegex(ValueError, "untouched questions changed"):
                load_bundle(root / "bundle")

    def test_incomplete_or_wrong_day_packet_is_not_an_evidence_ceiling(self):
        for changed in [
            {"fingerprint_complete":False},
            {"evidence_fingerprint":None},
            {"completeness":{"complete":False}},
            {"interval":{"from":"2026-09-06T00:00:00Z","to":"2026-09-07T00:00:00Z","timezone":"UTC"}},
        ]:
            value = packet() | changed
            with self.assertRaises(ValueError):
                validate_packet(value)

    def test_child_environment_excludes_service_credentials_and_routing(self):
        env = model_environment({"PATH":"/bin","HOME":"/owner","CODEX_HOME":"/untrusted","OPENAI_API_KEY":"secret","CODEX_API_KEY":"secret","OPENAI_BASE_URL":"https://gateway.invalid","BRUNN_PILOT_READ_TOKEN":"owner","HTTPS_PROXY":"https://proxy.invalid","CODEX_MODEL_PROVIDER":"custom","LANG":"C"},Path("/dedicated"),Path("/isolated"))
        self.assertEqual(env, {"PATH":"/bin","LANG":"C","HOME":"/isolated","CODEX_HOME":"/dedicated","NO_COLOR":"1"})

    def test_auth_preflight_requires_the_exact_chatgpt_login(self):
        with patch("scripts.location_pilot.subprocess.run", return_value=SimpleNamespace(returncode=0,stdout="Logged in using an API key",stderr="")):
            with self.assertRaisesRegex(ValueError,"ChatGPT-plan login"):
                verify_subscription(Path("/codex"),{})
        with patch("scripts.location_pilot.subprocess.run", side_effect=[SimpleNamespace(returncode=0,stdout="",stderr="Logged in using ChatGPT\n"),SimpleNamespace(returncode=0,stdout="codex-cli qualified\n",stderr="")]):
            self.assertEqual(verify_subscription(Path("/codex"),{}),"codex-cli qualified")

    def test_summary_arm_requires_an_exact_source_followup(self):
        plan = [{"path":"/v1/workspace/read","body":{"requests":[{"path":"derived/location/2026-09-06.md","view":"current_state"}]}}]
        with self.assertRaisesRegex(ValueError,"exact-version"):
            validate_plan("B",plan)
        plan.append({"path":"/v1/workspace/read","body":{"requests":[{"ref":"entry:synthetic","version":2,"view":"range","start":1,"end":4}]}})
        validate_plan("B",plan)
        with self.assertRaisesRegex(ValueError,"baseline"):
            validate_plan("A",plan)
        for path in ["/v1/workspace/write","/v1/location/rederive","https://foreign.invalid/v1/workspace/read"]:
            with self.assertRaises(ValueError):
                validate_plan("A",[{"path":path,"body":{}}])

    def test_unresolved_template_cannot_reach_the_service(self):
        with self.assertRaisesRegex(ValueError,"unresolved"):
            validate_plan("A",[{"path":"/v1/workspace/read","body":{"requests":[{"ref":"entry:REPLACE_WITH_FROZEN_CANONICAL_REF","version":1}]}}])


if __name__ == "__main__":
    unittest.main()
