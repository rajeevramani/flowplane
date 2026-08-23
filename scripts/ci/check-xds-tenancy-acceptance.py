#!/usr/bin/env python3
"""Acceptance gate for fpv2-d23.5 per-team xDS/SDS and telemetry isolation."""
from __future__ import annotations
import argparse, copy, json, os, re, sys, uuid
from pathlib import Path
from typing import Any, Callable, NoReturn

DEFAULT_EVIDENCE=Path('.artifacts/qualification/fpv2-d23.5/xds-tenancy.json')
DIGEST=re.compile(r'^sha256:[0-9a-f]{64}$')
UUID=re.compile(r'^[0-9a-f]{8}-[0-9a-f]{4}-[47][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',re.I)
FINGERPRINT=re.compile(r'^sha256:[0-9a-f]{64}$')
TEAMS=(('alpha','payments'),('alpha','shared'),('beta','payments'),('beta','shared'))
PROHIBITED_KEYS={'token','access_token','refresh_token','private_key','private_key_pem','certificate_pem','ca_certificate_pem','auth_key','password','secret_value','raw_config_dump'}
SECRET_PATTERNS=(re.compile(r'-----BEGIN'),re.compile(r'tskey-[A-Za-z0-9_-]+'),re.compile(r'(?i)bearer\s+[A-Za-z0-9._-]+'),re.compile(r'(?i)client_secret\s*[:=]'))

class ContractFailure(AssertionError): pass
def fail(message:str)->NoReturn: raise ContractFailure(message)
def obj(v:Any,n:str)->dict[str,Any]:
 if not isinstance(v,dict): fail(f'{n}: object required')
 return v
def seq(v:Any,n:str)->list[Any]:
 if not isinstance(v,list): fail(f'{n}: array required')
 return v
def field(root:dict[str,Any],path:str)->Any:
 v:Any=root
 for part in path.split('.'):
  v=obj(v,path).get(part)
  if v is None: fail(f'{path}: required')
 return v
def equal(root:dict[str,Any],path:str,want:Any)->None:
 got=field(root,path)
 if got!=want: fail(f'{path}: expected {want!r}, observed {got!r}')
def text(v:Any,n:str)->str:
 if not isinstance(v,str) or not v.strip(): fail(f'{n}: non-empty text required')
 return v
def indexed(v:Any,n:str)->dict[str,dict[str,Any]]:
 out={}
 for i,x in enumerate(seq(v,n)):
  x=obj(x,f'{n}[{i}]');k=text(x.get('id'),f'{n}[{i}].id')
  if k in out: fail(f'{n}: duplicate id {k!r}')
  out[k]=x
 return out

def check_candidate(e):
 equal(e,'candidate.source_commit','1cdf2af8fa08e0807298b665c22705884d360b5a');equal(e,'candidate.release','3.1.3')
 if not DIGEST.fullmatch(text(field(e,'candidate.image_digest'),'candidate.image_digest')): fail('candidate.image_digest: sha256 required')
 equal(e,'candidate.agent.version','3.1.3')
 if not DIGEST.fullmatch(text(field(e,'candidate.agent.sha256'),'candidate.agent.sha256')): fail('candidate.agent.sha256: sha256 required')
 if not re.fullmatch(r'1\.37\.5(?:\.\d+)?',text(field(e,'candidate.envoy.version'),'candidate.envoy.version')): fail('candidate.envoy.version: pinned 1.37.5 required')
 if not DIGEST.fullmatch(text(field(e,'candidate.envoy.image_digest'),'candidate.envoy.image_digest')): fail('candidate.envoy.image_digest: sha256 required')

def vm_map(e): return indexed(field(e,'vms'),'vms')
def stream_map(e): return indexed(field(e,'streams'),'streams')
def expected_id(org,team): return f'{org}-{team}'

def check_four_vms(e):
 equal(e,'run.simultaneous_four_vm_baseline',True);equal(e,'run.sequential_fallback',False)
 vms=vm_map(e);expected={expected_id(*x) for x in TEAMS}
 if set(vms)!=expected: fail(f'vms: exact four-team set required, got {sorted(vms)}')
 machine=set();nodes=set()
 for org,team in TEAMS:
  k=expected_id(org,team);v=vms[k]
  for f in ('org','team'):
   if v.get(f)!=(org if f=='org' else team): fail(f'vms[{k}].{f}: wrong tenancy')
  if v.get('running') is not True or v.get('linux') is not True or v.get('arch')!='x86_64': fail(f'vms[{k}]: running x86_64 Linux required')
  if v.get('host_mount_count')!=0 or v.get('foreign_credential_count')!=0: fail(f'vms[{k}]: host/foreign material present')
  for name,bucket in [('machine_fingerprint',machine),('tailscale_node_fingerprint',nodes)]:
   value=text(v.get(name),f'vms[{k}].{name}')
   if not FINGERPRINT.fullmatch(value): fail(f'vms[{k}].{name}: SHA-256 fingerprint required')
   if value in bucket: fail(f'vms[{k}].{name}: duplicate identity')
   bucket.add(value)
  if v.get('tailscale_online') is not True or v.get('tailscale_ephemeral') is not True: fail(f'vms[{k}]: ephemeral tailnet enrollment required')

def check_artifacts(e):
 for k,v in vm_map(e).items():
  if v.get('agent_version')!='3.1.3' or v.get('agent_sha256')!=field(e,'candidate.agent.sha256'): fail(f'vms[{k}]: exact agent mismatch')
  if v.get('envoy_version')!=field(e,'candidate.envoy.version') or v.get('envoy_image_digest')!=field(e,'candidate.envoy.image_digest'): fail(f'vms[{k}]: exact Envoy mismatch')
  if v.get('artifact_verified_before_start') is not True: fail(f'vms[{k}]: artifact verification absent')

def check_certificate_binding(e):
 streams=stream_map(e)
 if set(streams)!={expected_id(*x) for x in TEAMS}: fail('streams: exact four-team set required')
 certs=set();dps=set()
 for org,team in TEAMS:
  k=expected_id(org,team);s=streams[k]
  if s.get('org')!=org or s.get('team')!=team or s.get('certificate_bound_org')!=org or s.get('certificate_bound_team')!=team: fail(f'streams[{k}]: certificate tenancy mismatch')
  if s.get('registered') is not True or s.get('certificate_active') is not True or s.get('spiffe_registry_match') is not True: fail(f'streams[{k}]: active registry binding absent')
  for f,bucket in [('certificate_fingerprint',certs),('dataplane_fingerprint',dps)]:
   value=text(s.get(f),f'streams[{k}].{f}')
   if not FINGERPRINT.fullmatch(value) or value in bucket: fail(f'streams[{k}].{f}: unique fingerprint required')
   bucket.add(value)

def check_ads(e):
 for k,s in stream_map(e).items():
  ads=obj(s.get('ads'),f'streams[{k}].ads')
  if ads.get('connected') is not True or ads.get('cp_stream_observed') is not True or ads.get('ack_observed') is not True: fail(f'streams[{k}].ads: connected ACK proof required')
  if ads.get('own_resource_count',0)<3 or ads.get('foreign_resource_count')!=0: fail(f'streams[{k}].ads: own resources plus zero foreign required')
  if ads.get('foreign_names_observed')!=[]: fail(f'streams[{k}].ads: foreign names leaked')
  if ads.get('version_fingerprint') is None: fail(f'streams[{k}].ads: version fingerprint required')

def check_sds(e):
 for k,s in stream_map(e).items():
  sds=obj(s.get('sds'),f'streams[{k}].sds')
  if sds.get('delivered') is not True or sds.get('ack_observed') is not True: fail(f'streams[{k}].sds: delivered ACK required')
  if sds.get('own_secret_count',0)<1 or sds.get('foreign_secret_count')!=0: fail(f'streams[{k}].sds: own secret plus zero foreign required')
  if sds.get('secret_values_recorded') is not False or sds.get('private_keys_recorded') is not False: fail(f'streams[{k}].sds: secret material recorded')
  if sds.get('foreign_secret_versions_observed')!=[]: fail(f'streams[{k}].sds: foreign secret versions leaked')

def check_status(e):
 for k,s in stream_map(e).items():
  status=obj(s.get('status'),f'streams[{k}].status')
  expected=s.get('dataplane_fingerprint')
  if status.get('xds_connected') is not True or status.get('heartbeat_advanced') is not True or status.get('config_verify_advanced') is not True: fail(f'streams[{k}].status: xDS/diagnostics disagreement')
  if status.get('observable_dataplane_fingerprints')!=[expected,expected,expected]: fail(f'streams[{k}].status: identity mismatch')
  if status.get('foreign_dataplane_count')!=0: fail(f'streams[{k}].status: foreign dataplane visible')

def check_projection(e):
 projections=indexed(field(e,'config_projections'),'config_projections')
 if set(projections)!={expected_id(*x) for x in TEAMS}: fail('config_projections: exact four-team set required')
 for k,p in projections.items():
  if p.get('source')!='envoy_loopback_admin' or p.get('test_only') is not True or p.get('raw_dump_retained') is not False: fail(f'config_projections[{k}]: test-only redacted loopback source required')
  if p.get('fields')!=['resource_type','resource_name_fingerprint','version_fingerprint','structural_hash']: fail(f'config_projections[{k}].fields: structural allowlist required')
  if p.get('own_resource_count',0)<3 or p.get('foreign_resource_count')!=0 or p.get('foreign_secret_version_count')!=0: fail(f'config_projections[{k}]: foreign structure leaked')
  if p.get('inline_secret_count')!=0 or p.get('typed_secret_value_count')!=0: fail(f'config_projections[{k}]: secret material retained')

def check_traffic(e):
 probes=indexed(field(e,'traffic'),'traffic')
 if set(probes)!={expected_id(*x) for x in TEAMS}: fail('traffic: exact four-team set required')
 for k,p in probes.items():
  if p.get('own_listener_status')!=200 or p.get('own_upstream_marker')!=p.get('response_marker'): fail(f'traffic[{k}]: own path failed')
  if p.get('foreign_listener_reachable_count')!=0 or p.get('foreign_marker_observed_count')!=0 or p.get('foreign_effect_count')!=0: fail(f'traffic[{k}]: foreign behavior observed')
  if p.get('cp_in_request_path') is not False: fail(f'traffic[{k}]: CP entered request path')

def check_telemetry(e):
 rows=indexed(field(e,'telemetry'),'telemetry')
 if set(rows)!={expected_id(*x) for x in TEAMS}: fail('telemetry: exact four-team set required')
 for k,p in rows.items():
  for f in ('heartbeat','stats','nacks','audit'):
   x=obj(p.get(f),f'telemetry[{k}].{f}')
   if x.get('team_scoped') is not True or x.get('foreign_row_count')!=0: fail(f'telemetry[{k}].{f}: foreign rows visible')
  if p.get('raw_identity_recorded') is not False: fail(f'telemetry[{k}]: raw identity recorded')

def check_rotation(e):
 rows=indexed(field(e,'certificate_lifecycle'),'certificate_lifecycle')
 for org,team in TEAMS:
  k='rotation-'+expected_id(org,team)
  if k not in rows: fail(f'certificate_lifecycle: missing {k}')
  x=rows[k]
  if x.get('old_and_new_overlap_proven') is not True or x.get('new_identity_connected') is not True or x.get('new_identity_ack_observed') is not True or x.get('old_identity_revoked_after_cutover') is not True: fail(f'certificate_lifecycle[{k}]: safe rotation not proven')
  if x.get('foreign_config_count')!=0 or x.get('foreign_telemetry_write_count')!=0: fail(f'certificate_lifecycle[{k}]: foreign effect')

def check_negative_identities(e):
 rows=indexed(field(e,'identity_negatives'),'identity_negatives')
 required={'expired','revoked','wrong_team','unregistered','malformed','wrong_server_trust'}
 if set(rows)!=required: fail('identity_negatives: exact negative matrix required')
 for k,x in rows.items():
  if k in {'expired','revoked','unregistered'}:
   if x.get('evidence_source')!='real_db_ads_test' or x.get('test_passed') is not True:
    fail(f'identity_negatives[{k}]: passing real-DB ADS test evidence required')
   expected=('fp-xds::ads_mtls::registry_binds_team_and_revocation_kills_live_stream' if k=='revoked' else 'fp-xds::ads_mtls::unregistered_and_expired_certificates_are_rejected')
   if text(x.get('test_ref'),f'identity_negatives[{k}].test_ref') != expected:
    fail(f'identity_negatives[{k}].test_ref: wrong focused test')
  elif x.get('evidence_source')!='live_vm':
   fail(f'identity_negatives[{k}]: live VM evidence required')
  if x.get('connected') is not False or x.get('config_delivered') is not False or x.get('telemetry_committed') is not False or x.get('foreign_effect_count')!=0: fail(f'identity_negatives[{k}]: failure did not close')
  if x.get('bounded_timeout') is not True or x.get('outcome') not in {'tls_rejected','registry_rejected','expired_rejected','revoked_disconnected'}: fail(f'identity_negatives[{k}]: classified bounded outcome required')

def check_vm_isolation(e):
 for k,v in vm_map(e).items():
  if v.get('credential_files_owned_by_team') is not True or v.get('credential_file_mode')!='0600': fail(f'vms[{k}]: credential ownership/mode invalid')
  if v.get('other_team_names_in_filesystem_scan')!=0 or v.get('other_team_credentials_in_process_env')!=0: fail(f'vms[{k}]: cross-team residue')
  if v.get('envoy_admin_loopback_only') is not True or v.get('agent_health_loopback_only') is not True: fail(f'vms[{k}]: local admin surface exposed')

def check_freeze_cleanup(e):
 equal(e,'cleanup.evidence_frozen_before_cleanup',True);equal(e,'cleanup.safe_to_rerun',True)
 inv=indexed(field(e,'cleanup.inventories'),'cleanup.inventories')
 required={'lima_vms','tailscale_nodes','tailscale_keys','dataplanes','certificates','clusters','listeners','routes','secrets'}
 if set(inv)!=required: fail(f'cleanup.inventories: expected {sorted(required)}')
 frozen=text(field(e,'cleanup.frozen_at_utc'),'cleanup.frozen_at_utc');completed=text(field(e,'cleanup.completed_at_utc'),'cleanup.completed_at_utc')
 if completed<=frozen: fail('cleanup: completion must post-date evidence freeze')
 for k,x in inv.items():
  if x.get('run_owned_remaining_count')!=0 or x.get('exact_registered_objects_checked') is not True: fail(f'cleanup.inventories[{k}]: residue or incomplete inventory')
  text(x.get('authoritative_source'),f'cleanup.inventories[{k}].authoritative_source');text(x.get('observed_at_utc'),f'cleanup.inventories[{k}].observed_at_utc')

def walk(v:Any,path='$'):
 if isinstance(v,dict):
  for k,x in v.items():
   if k.lower() in PROHIBITED_KEYS: fail(f'redaction: prohibited key {path}.{k}')
   walk(x,f'{path}.{k}')
 elif isinstance(v,list):
  for i,x in enumerate(v): walk(x,f'{path}[{i}]')
 elif isinstance(v,str):
  for p in SECRET_PATTERNS:
   if p.search(v): fail(f'redaction: prohibited value at {path}')
def check_redaction(e):
 equal(e,'redaction.sanitized_projection',True);equal(e,'redaction.raw_identifiers_recorded',False);equal(e,'redaction.secret_values_recorded',False);equal(e,'redaction.private_paths_recorded',False);equal(e,'redaction.scan_after_final_edit',True);equal(e,'redaction.undisposed_match_count',0)
 classes=set(seq(field(e,'redaction.pattern_classes'),'redaction.pattern_classes'))
 if not {'private_keys','certificate_bodies','tailscale_keys','bearer_tokens','private_paths','raw_config_dump','foreign_identifiers'}.issubset(classes): fail('redaction.pattern_classes: incomplete')
 walk(e)

SCENARIOS={
 'FPV2-D23.5-CANDIDATE':('exact candidate and pinned artifacts',check_candidate),
 'FPV2-D23.5-FOUR-VMS':('four simultaneous isolated Linux trust domains',check_four_vms),
 'FPV2-D23.5-ARTIFACTS':('exact agent and Envoy in every VM',check_artifacts),
 'FPV2-D23.5-CERT-BINDING':('certificate registry binds each stream to one team',check_certificate_binding),
 'FPV2-D23.5-ADS':('ADS contains only certificate-bound team resources',check_ads),
 'FPV2-D23.5-SDS':('SDS contains only bound-team structural secret evidence',check_sds),
 'FPV2-D23.5-ACK-STATUS':('CP stream ACK and diagnostics agree',check_status),
 'FPV2-D23.5-CONFIG-PROJECTION':('test-only redacted loopback structural projection',check_projection),
 'FPV2-D23.5-TRAFFIC':('tagged traffic and foreign non-effect',check_traffic),
 'FPV2-D23.5-TELEMETRY':('telemetry stats NACKs and audit stay team-scoped',check_telemetry),
 'FPV2-D23.5-ROTATION':('safe certificate rotation and reconnect',check_rotation),
 'FPV2-D23.5-NEGATIVE-IDENTITIES':('identity and trust failures close',check_negative_identities),
 'FPV2-D23.5-VM-ISOLATION':('VM filesystems credentials and local admin isolation',check_vm_isolation),
 'FPV2-D23.5-CLEANUP-REDACTION':('freeze cleanup residue and redaction',lambda e:(check_freeze_cleanup(e),check_redaction(e))),
}

def fp(n:int)->str:return 'sha256:'+f'{n:064x}'
def fixture()->dict[str,Any]:
 vms=[];streams=[];projections=[];traffic=[];telemetry=[];lifecycle=[]
 for i,(org,team) in enumerate(TEAMS,1):
  k=expected_id(org,team)
  vms.append({'id':k,'org':org,'team':team,'running':True,'linux':True,'arch':'x86_64','host_mount_count':0,'foreign_credential_count':0,'machine_fingerprint':fp(i),'tailscale_node_fingerprint':fp(10+i),'tailscale_online':True,'tailscale_ephemeral':True,'agent_version':'3.1.3','agent_sha256':fp(100),'envoy_version':'1.37.5','envoy_image_digest':fp(101),'artifact_verified_before_start':True,'credential_files_owned_by_team':True,'credential_file_mode':'0600','other_team_names_in_filesystem_scan':0,'other_team_credentials_in_process_env':0,'envoy_admin_loopback_only':True,'agent_health_loopback_only':True})
  streams.append({'id':k,'org':org,'team':team,'certificate_bound_org':org,'certificate_bound_team':team,'registered':True,'certificate_active':True,'spiffe_registry_match':True,'certificate_fingerprint':fp(20+i),'dataplane_fingerprint':fp(30+i),'ads':{'connected':True,'cp_stream_observed':True,'ack_observed':True,'own_resource_count':4,'foreign_resource_count':0,'foreign_names_observed':[],'version_fingerprint':fp(40+i)},'sds':{'delivered':True,'ack_observed':True,'own_secret_count':1,'foreign_secret_count':0,'secret_values_recorded':False,'private_keys_recorded':False,'foreign_secret_versions_observed':[]},'status':{'xds_connected':True,'heartbeat_advanced':True,'config_verify_advanced':True,'observable_dataplane_fingerprints':[fp(30+i)]*3,'foreign_dataplane_count':0}})
  projections.append({'id':k,'source':'envoy_loopback_admin','test_only':True,'raw_dump_retained':False,'fields':['resource_type','resource_name_fingerprint','version_fingerprint','structural_hash'],'own_resource_count':4,'foreign_resource_count':0,'foreign_secret_version_count':0,'inline_secret_count':0,'typed_secret_value_count':0})
  traffic.append({'id':k,'own_listener_status':200,'own_upstream_marker':f'marker-{k}','response_marker':f'marker-{k}','foreign_listener_reachable_count':0,'foreign_marker_observed_count':0,'foreign_effect_count':0,'cp_in_request_path':False})
  telemetry.append({'id':k,'heartbeat':{'team_scoped':True,'foreign_row_count':0},'stats':{'team_scoped':True,'foreign_row_count':0},'nacks':{'team_scoped':True,'foreign_row_count':0},'audit':{'team_scoped':True,'foreign_row_count':0},'raw_identity_recorded':False})
  lifecycle.append({'id':'rotation-'+k,'old_and_new_overlap_proven':True,'new_identity_connected':True,'new_identity_ack_observed':True,'old_identity_revoked_after_cutover':True,'foreign_config_count':0,'foreign_telemetry_write_count':0})
 negatives=[{'id':k,'connected':False,'config_delivered':False,'telemetry_committed':False,'foreign_effect_count':0,'bounded_timeout':True,'outcome':o} for k,o in [('expired','expired_rejected'),('revoked','revoked_disconnected'),('wrong_team','registry_rejected'),('unregistered','registry_rejected'),('malformed','tls_rejected'),('wrong_server_trust','tls_rejected')]]
 for item in negatives:
  if item['id'] in {'expired','revoked','unregistered'}:
   item.update(evidence_source='real_db_ads_test',test_ref=('fp-xds::ads_mtls::registry_binds_team_and_revocation_kills_live_stream' if item['id']=='revoked' else 'fp-xds::ads_mtls::unregistered_and_expired_certificates_are_rejected'),test_passed=True)
  else:
   item['evidence_source']='live_vm'
 inv=[{'id':k,'run_owned_remaining_count':0,'exact_registered_objects_checked':True,'authoritative_source':'supported_inventory','observed_at_utc':'2026-08-20T03:00:00Z'} for k in ('lima_vms','tailscale_nodes','tailscale_keys','dataplanes','certificates','clusters','listeners','routes','secrets')]
 return {'candidate':{'source_commit':'1cdf2af8fa08e0807298b665c22705884d360b5a','release':'3.1.3','image_digest':fp(99),'agent':{'version':'3.1.3','sha256':fp(100)},'envoy':{'version':'1.37.5','image_digest':fp(101)}},'run':{'simultaneous_four_vm_baseline':True,'sequential_fallback':False},'vms':vms,'streams':streams,'config_projections':projections,'traffic':traffic,'telemetry':telemetry,'certificate_lifecycle':lifecycle,'identity_negatives':negatives,'cleanup':{'evidence_frozen_before_cleanup':True,'safe_to_rerun':True,'frozen_at_utc':'2026-08-20T02:59:00Z','completed_at_utc':'2026-08-20T03:00:00Z','inventories':inv},'redaction':{'sanitized_projection':True,'raw_identifiers_recorded':False,'secret_values_recorded':False,'private_paths_recorded':False,'scan_after_final_edit':True,'undisposed_match_count':0,'pattern_classes':['private_keys','certificate_bodies','tailscale_keys','bearer_tokens','private_paths','raw_config_dump','foreign_identifiers']}}

def self_test()->int:
 good=fixture();failures=[]
 for sid,(_,check) in SCENARIOS.items():
  try:check(good)
  except ContractFailure as e:failures.append(f'valid fixture rejected by {sid}: {e}')
 mutations=[
  ('duplicate VM identity',lambda x:x['vms'][1].__setitem__('machine_fingerprint',x['vms'][0]['machine_fingerprint']),'FPV2-D23.5-FOUR-VMS'),
  ('host mount',lambda x:x['vms'][0].__setitem__('host_mount_count',1),'FPV2-D23.5-FOUR-VMS'),
  ('wrong agent',lambda x:x['vms'][0].__setitem__('agent_version','3.1.2'),'FPV2-D23.5-ARTIFACTS'),
  ('certificate cross-team',lambda x:x['streams'][0].__setitem__('certificate_bound_team','shared'),'FPV2-D23.5-CERT-BINDING'),
  ('foreign ADS resource',lambda x:x['streams'][0]['ads'].__setitem__('foreign_resource_count',1),'FPV2-D23.5-ADS'),
  ('SDS value recorded',lambda x:x['streams'][0]['sds'].__setitem__('secret_values_recorded',True),'FPV2-D23.5-SDS'),
  ('missing ACK',lambda x:x['streams'][0]['ads'].__setitem__('ack_observed',False),'FPV2-D23.5-ADS'),
  ('raw dump retained',lambda x:x['config_projections'][0].__setitem__('raw_dump_retained',True),'FPV2-D23.5-CONFIG-PROJECTION'),
  ('foreign traffic marker',lambda x:x['traffic'][0].__setitem__('foreign_marker_observed_count',1),'FPV2-D23.5-TRAFFIC'),
  ('foreign telemetry',lambda x:x['telemetry'][0]['stats'].__setitem__('foreign_row_count',1),'FPV2-D23.5-TELEMETRY'),
  ('unsafe rotation',lambda x:x['certificate_lifecycle'][0].__setitem__('old_identity_revoked_after_cutover',False),'FPV2-D23.5-ROTATION'),
  ('negative connected',lambda x:x['identity_negatives'][0].__setitem__('connected',True),'FPV2-D23.5-NEGATIVE-IDENTITIES'),
  ('revoked evidence uses unverified live status',lambda x:next(v for v in x['identity_negatives'] if v['id']=='revoked').__setitem__('evidence_source','live_vm'),'FPV2-D23.5-NEGATIVE-IDENTITIES'),
  ('foreign credential file',lambda x:x['vms'][0].__setitem__('other_team_names_in_filesystem_scan',1),'FPV2-D23.5-VM-ISOLATION'),
  ('cleanup residue',lambda x:x['cleanup']['inventories'][0].__setitem__('run_owned_remaining_count',1),'FPV2-D23.5-CLEANUP-REDACTION'),
  ('secret shaped field',lambda x:x.__setitem__('private_key','forbidden'),'FPV2-D23.5-CLEANUP-REDACTION'),
 ]
 for label,mut,sid in mutations:
  bad=copy.deepcopy(good);mut(bad)
  try:SCENARIOS[sid][1](bad)
  except ContractFailure:pass
  else:failures.append(f'negative self-test did not fail closed: {label}')
 if failures:
  for x in failures:print('SELF-TEST FAIL:',x,file=sys.stderr)
  print(f'xds tenancy acceptance self-test: FAIL ({len(failures)} failures)',file=sys.stderr);return 1
 print(f'xds tenancy acceptance self-test: PASS ({len(SCENARIOS)} scenarios, {len(mutations)} fail-closed mutations)');return 0

def load(path:Path)->dict[str,Any]:
 try:return obj(json.loads(path.read_text()),'evidence root')
 except (OSError,UnicodeError,json.JSONDecodeError) as e:fail(f'evidence unreadable: {e}')
def args():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence');p.add_argument('--scenario',choices=sorted(SCENARIOS));p.add_argument('--list',action='store_true');p.add_argument('--self-test',action='store_true');return p.parse_args()
def main()->int:
 a=args()
 if a.list:
  for k,(d,_) in SCENARIOS.items():print(f'{k}\t{d}')
  return 0
 if a.self_test:return self_test()
 path=Path(a.evidence or os.environ.get('FLOWPLANE_XDS_TENANCY_EVIDENCE',DEFAULT_EVIDENCE));selected=[a.scenario] if a.scenario else list(SCENARIOS)
 try:e=load(path)
 except ContractFailure as err:
  print(f'xds tenancy acceptance: FAIL: {err}',file=sys.stderr);return 1
 failures=0
 for sid in selected:
  desc,check=SCENARIOS[sid]
  try:check(e)
  except ContractFailure as err:failures+=1;print(f'{sid}: FAIL: {err}',file=sys.stderr)
  else:print(f'{sid}: PASS: {desc}')
 if failures:print(f'xds tenancy acceptance: FAIL ({failures}/{len(selected)} scenarios)',file=sys.stderr);return 1
 print(f'xds tenancy acceptance: PASS ({len(selected)} scenarios)');return 0
if __name__=='__main__':raise SystemExit(main())
