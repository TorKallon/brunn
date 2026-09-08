#!/usr/bin/env python3
"""Operator preparation/provisioning for one Brunn Railway release.

Default preflight is read-only. Mutations require an explicit subcommand,
--execute, a committed clean main SHA, and the deployed API revision. No token
is printed, placed in argv, or written to local files. No deployment is started.
"""
from __future__ import annotations
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]
PROJECT = "20f62209-77fd-4484-8101-88f8c2022456"
OWNER = "0ed8980c-d469-4e0f-857b-0c11a379b5fa"
OWNER_REF = "user:" + OWNER
ACTOR = "87b30ef5-8b0a-4fc6-943c-8b00563176c5"
RUNNER = "089d1916-dfe3-454d-85cd-491a1e3e17ae"
MODEL_SECRET = "dreamer-model-credential"
MODEL_CAPS = {"open","query","read","compute","verify","status","task.read","message.read"}
SPEC = importlib.util.spec_from_file_location("dreamer_canary", ROOT / "scripts/dreamer-production-canary.py")
CANARY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CANARY)
require, Error = CANARY.require, CANARY.CanaryError

# Railway SSH carries only this constant program in argv. The owner token and
# optional fresh-user request travel through stdin. Its secret-bearing stdout
# is captured in a Python pipe and is never copied to the terminal or a file.
PRIVATE_NODE = r"""
(async()=>{let raw='';for await(const part of process.stdin){raw+=part;if(raw.length>16384)throw Error('input_bound');}
const input=JSON.parse(raw);const base='http://api.railway.internal:8080';
async function call(path,body){const response=await fetch(base+path,{method:body?'POST':'GET',redirect:'error',signal:AbortSignal.timeout(30000),headers:{Authorization:'Bearer '+input.owner_token,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});if(!response.ok)throw Error('private_http_'+response.status);const text=await response.text();if(text.length>65536)throw Error('response_bound');return JSON.parse(text);}
const me=await call('/v1/me');if(me.user.id!=='user:0ed8980c-d469-4e0f-857b-0c11a379b5fa'||!me.capabilities.includes('admin'))throw Error('owner_mismatch');
if(input.action==='probe'){process.stdout.write(JSON.stringify({owner_verified:true,private_api_reachable:true}));return;}
if(input.action!=='provision'||!/^brunn-dreamer-canary:[a-f0-9-]{36}$/.test(input.fixture.external_ref))throw Error('fixture_refused');
const result=await call('/v1/admin/users',input.fixture);process.stdout.write(JSON.stringify(result));
})().catch(()=>{process.stderr.write('Private provisioning failed or uncertain; do not retry blindly.\n');process.exitCode=1;});
"""


def railway(args, command, *, payload=None):
    result = subprocess.run([str(args.railway), *command], cwd=ROOT, input=payload,
        capture_output=True, timeout=90)
    require(result.returncode == 0, "Railway command failed or uncertain; raw output withheld")
    return result.stdout


def scoped(service):
    return ["--project",PROJECT,"--service",service,"--environment","production"]


def db_sql(args, sql):
    remote = " ".join(map(shlex.quote,["psql","-X","-qAt","-v","ON_ERROR_STOP=1","-U","admin","-d","straylight","-c",sql]))
    raw = railway(args,["ssh",*scoped("db"),remote])
    return [json.loads(line) for line in raw.decode().splitlines() if line.startswith("{")]


def variables(args, service):
    return json.loads(railway(args,["variable","list",*scoped(service),"--json"]))


def stage_variable(args, service, key, value):
    # --skip-deploys is mandatory, including for read-preference flags.
    railway(args,["variable","set",key,*scoped(service),"--stdin","--skip-deploys"],payload=value.encode())
    require(variables(args,service).get(key)==value,"Railway staged-value readback failed; do not deploy")


def owner_client(report):
    return CANARY.Client("https://brunn.ai/api",CANARY.owner_token(),report["calls"],"owner")


def deployed_gate(owner, revision):
    me=owner.request("GET","/v1/me")
    require(me["user"]["id"]==OWNER_REF and "admin" in me["capabilities"] and "credential:manage" in me["capabilities"],"owner identity/capability mismatch")
    status=owner.request("GET","/v1/status")
    require(status["build_revision"]==revision,"API has not deployed the exact committed release")
    require(owner.request("GET","/ready",public=True).get("status")=="ready","API readiness gate failed")
    return status


def private_request(args, owner, action, fixture=None):
    remote=" ".join(map(shlex.quote,["node","-e",PRIVATE_NODE]))
    payload=json.dumps({"owner_token":owner.token,"action":action,"fixture":fixture}).encode()
    return json.loads(railway(args,["ssh",*scoped("dreamer"),remote],payload=payload))


def preflight(args, report):
    state=json.loads(railway(args,["status","--json"]))
    require(state["id"]==PROJECT,"linked project is not Brunn")
    report["deployments"]=[]
    for environment in state["environments"]["edges"]:
        if environment["node"]["name"]!="production":continue
        for edge in environment["node"]["serviceInstances"]["edges"]:
            service=edge["node"]; deployment=service["latestDeployment"];meta=deployment.get("meta",{})
            if service["serviceName"] not in {"worker","api","mcp","web","dreamer","db"}:continue
            config=meta.get("serviceManifest",{})
            report["deployments"].append({"service":service["serviceName"],"deployment_id":deployment["id"],"status":deployment["status"],"digest":meta.get("imageDigest"),"dockerfile":config.get("build",{}).get("dockerfilePath"),"predeploy":config.get("deploy",{}).get("preDeployCommand")})
    report["database"]=db_sql(args,"BEGIN READ ONLY; SELECT json_build_object('database',current_database(),'role',current_user,'migration',(SELECT max(version) FROM _sqlx_migrations)); SELECT json_build_object('credential_id',id,'owner_id',user_id,'capabilities',capabilities,'active',disabled_at IS NULL) FROM brunn.api_credentials WHERE id='"+RUNNER+"'; ROLLBACK;")
    report["dreamer_variable_names"]=sorted(variables(args,"dreamer"))
    owner=owner_client(report)
    report["private_transport"]=private_request(args,owner,"probe")
    report["status"]="read_only_preflight_complete"


def clean_release(revision):
    require(bool(re.fullmatch(r"[0-9a-f]{40}",revision or "")),"expected revision must be a full lowercase commit SHA")
    def git(*command):return subprocess.check_output(["git",*command],cwd=ROOT,text=True).strip()
    require(git("branch","--show-current")=="main" and git("rev-parse","HEAD")==revision and not git("status","--porcelain"),"mutation requires the clean committed main release")


def grant_runner(args, owner, report):
    deployed_gate(owner,args.expected_revision)
    sql=(ROOT/"scripts/dreamer-runner-grant.sql").read_text().replace("__RELEASE_REVISION__",args.expected_revision)
    report["grant"]=db_sql(args,sql)
    require(len(report["grant"])==1 and set(report["grant"][0]["capabilities"])=={"secret:read","secret:write","notification:publish","dreamer:run"},"runner capability readback mismatch")
    report["status"]="runner_grant_verified"


def verify_model(owner, token):
    model=CANARY.Client(owner.base,token,owner.calls,"model_identity")
    me=model.request("GET","/v1/me")
    require(me["user"]["id"]==OWNER_REF and me.get("read_only") is True and "read" in me["capabilities"] and set(me["capabilities"])<=MODEL_CAPS,"model credential is not the dedicated read-only owner credential")
    return me


def stage_model(args,owner,report,persist):
    deployed_gate(owner,args.expected_revision)
    current=variables(args,"dreamer")
    if "DREAMER_MODEL_TOKEN" in current:
        require(bool(current["DREAMER_MODEL_TOKEN"]),"existing sealed/empty model variable requires operator inspection; refusing overwrite")
        verify_model(owner,current["DREAMER_MODEL_TOKEN"])
        report["status"]="existing_model_variable_verified"
        return
    secret=owner.request("POST","/v1/workspace/secrets/get",{"name":MODEL_SECRET},expected=(200,404))
    if secret.get("http_status")==404:
        credential=owner.request("POST","/v1/credentials",{"name":"Dreamer model (read-only)","access":"read_only"})
        report["issued_credential_ref"]=credential["id"]
        persist()  # Credential identity is recovery metadata, never the token.
        custody={"schema":"dreamer.model-credential.v1","credential_ref":credential["id"],"owner_id":OWNER_REF,"token":credential["token"]}
        verify_model(owner,custody["token"])
        body={"name":MODEL_SECRET,"value":json.dumps(custody,separators=(",",":")),"expected_version":0,"description":"Dedicated read-only Dreamer model credential; operator custody for staged Railway deployment."}
        try:
            owner.request("POST","/v1/workspace/secrets/put",body)
        except Error:
            # A successful-but-lost response must resolve to these exact bytes.
            pass
        secret=owner.request("POST","/v1/workspace/secrets/get",{"name":MODEL_SECRET})
        require(secret.get("value")==body["value"],"model vault custody failed; inspect issued credential before retrying")
    custody=json.loads(secret["value"])
    require(custody.get("schema")=="dreamer.model-credential.v1" and custody.get("owner_id")==OWNER_REF,"model custody record identity mismatch")
    verify_model(owner,custody["token"])
    report["model_credential_ref"]=custody["credential_ref"]
    report["model_custody_secret_ref"]=secret["secret_ref"]
    stage_variable(args,"dreamer","DREAMER_MODEL_TOKEN",custody["token"])
    report["status"]="model_variable_staged_and_verified"


def execute_canary(args,owner,report,persist):
    status=deployed_gate(owner,args.expected_revision)
    required_flag=args.summary_preference=="enabled"
    require(status.get("feature_flags",{}).get("dreamer_summary_reads_enabled") is required_flag,"initial summary preference gate mismatch")
    owner.request("GET","/v1/dreamer/review")
    fixture={"external_ref":"brunn-dreamer-canary:"+str(uuid.uuid4()),"display_name":"Disposable Dreamer release canary","credential_name":"Canary cleanup owner"}
    report["fixture_external_ref"]=fixture["external_ref"]
    report["status"]="provisioning_requested"
    persist()  # Resolve this external ref if a private response is uncertain.
    provisioned=private_request(args,owner,"provision",fixture)
    fixture={"external_ref":provisioned["user"]["external_ref"],"user_id":provisioned["user"]["id"]}
    report["fixture_user_id"]=fixture["user_id"]
    persist()
    credential=CANARY.Client(owner.base,provisioned["credential"]["token"],report["calls"],"bridge_fixture_cleanup")
    rfd,wfd=-1,-1
    child=None
    try:
        rfd,wfd=os.pipe()
        command=[sys.executable,str(ROOT/"scripts/dreamer-production-canary.py"),"--execute","--expected-revision",args.expected_revision,"--fixture-fd",str(rfd),"--report",str(args.canary_report),"--summary-preference",args.summary_preference,"--flag-wait-seconds",str(args.flag_wait_seconds)]
        child=subprocess.Popen(command,cwd=ROOT,pass_fds=(rfd,))
        os.close(rfd);rfd=-1
        owned_wfd,wfd=wfd,-1
        with os.fdopen(owned_wfd,"wb") as output:
            output.write(json.dumps(provisioned).encode())
        report["canary_exit_code"]=child.wait()
    finally:
        for descriptor in (rfd,wfd):
            if descriptor>=0:
                try:os.close(descriptor)
                except OSError:pass
        if child is not None and child.poll() is None:
            child.terminate()
            try:child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill();child.wait()
        try:
            result=json.loads(args.canary_report.read_text()) if args.canary_report.exists() and args.canary_report.stat().st_size else {}
            if not isinstance(result,dict):result={}
        except (OSError,ValueError):result={}
        cleanup_result=result.get("cleanup") or {}
        if not isinstance(cleanup_result,dict):cleanup_result={}
        if not cleanup_result.get("canonical_purge_verified"):
            report["bridge_cleanup"]=CANARY.cleanup(credential,fixture,OWNER_REF,180)
        else:
            report["bridge_cleanup"]={"canonical_purge_verified":True,"performed_by":"canary"}
        report["status"]="canary_complete" if result.get("status")=="passed" else "canary_failed_or_cleanup_pending"


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action",choices=["preflight","grant-runner","stage-model","canary"],nargs="?",default="preflight")
    parser.add_argument("--execute",action="store_true")
    parser.add_argument("--expected-revision")
    parser.add_argument("--railway",type=Path,default=Path("/Users/aether/.npm-global/bin/railway"))
    parser.add_argument("--report",type=Path,required=True)
    parser.add_argument("--canary-report",type=Path)
    parser.add_argument("--summary-preference",choices=["enabled","disabled_then_enabled"],default="disabled_then_enabled")
    parser.add_argument("--flag-wait-seconds",type=int,default=600)
    args=parser.parse_args()
    require(not args.report.exists(),"report exists; preserve the prior rollout recovery journal")
    args.report.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
    fd=os.open(args.report,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600);os.close(fd)
    report={"schema":"brunn.dreamer-railway-rollout.v1","action":args.action,"status":"started","calls":[]}
    def persist():args.report.write_text(json.dumps(report,indent=2)+"\n")
    try:
        if args.action=="preflight":preflight(args,report)
        else:
            require(args.execute,"mutation requires --execute")
            clean_release(args.expected_revision)
            owner=owner_client(report)
            if args.action=="grant-runner":grant_runner(args,owner,report)
            elif args.action=="stage-model":stage_model(args,owner,report,persist)
            else:
                require(args.canary_report is not None and not args.canary_report.exists(),"canary requires a new --canary-report path")
                execute_canary(args,owner,report,persist)
    except Error as error:
        report["status"]="failed_or_uncertain";report["error"]=str(error)
    except (OSError,ValueError,KeyError,TypeError,subprocess.SubprocessError):
        report["status"]="failed_or_uncertain";report["error"]="Unexpected local or response failure; secret-bearing details withheld"
    finally:persist()
    print(json.dumps({"status":report["status"],"report":str(args.report),"action":args.action}))
    return 1 if report["status"] in {"failed_or_uncertain","canary_failed_or_cleanup_pending"} else 0


if __name__=="__main__":
    try:raise SystemExit(main())
    except Error as error:print(str(error),file=sys.stderr);raise SystemExit(1)
