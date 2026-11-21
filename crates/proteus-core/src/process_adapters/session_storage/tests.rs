use std::{path::Path, sync::Arc};

use crate::{
    contracts::{SessionStorage, SessionStorageRequest, SessionStorageResponse},
    core::{AppConfig, ModuleCatalog},
};
use serde_json::json;

const WORKER: &str = r#"import json, sys
log = sys.argv[1]
init = json.loads(sys.stdin.readline())
binding = init['params']
exports = [{ 'slot': e['slot'], 'module_id': e['module_id'],
 'contract_version': e['contract_version'], 'composition': e['composition'],
 'module_features': [], 'config_schema': None } for e in binding['exports']]
print(json.dumps({'jsonrpc':'2.0','id':init['id'],'result':{
 'protocol_version':'v3','component_id':binding['component_id'],'exports':exports}}), flush=True)
bad = binding['exports'][0]['module_config'].get('bad_response',False)
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    with open(log,'a') as output:
        output.write(req['method']+'\n')
    result = {'result':'done'} if bad else {'result':'sessions','sessions':[]}
    print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':result}),flush=True)
"#;

fn config(root: &Path, bad_response: bool) -> AppConfig {
    let script = root.join("storage.py");
    std::fs::write(&script, WORKER).unwrap();
    serde_json::from_value(json!({
        "modules":{"session_storage":"records"},
        "components":{
            "storage":{"command":"python3","args":[script,root.join("requests.log")],
                "exports":{"session_storage":{"records":{}}}},
            "unselected-model":{"command":"missing-unselected-model-worker",
                "exports":{"model":{"unused":{}}}}
        },
        "module_config":{"session_storage":{"records":{"bad_response":bad_response}}}
    }))
    .unwrap()
}

fn list(root: &Path) -> SessionStorageRequest {
    SessionStorageRequest::List {
        config_root: root.to_owned(),
        workspace_path: None,
        audit: false,
    }
}

fn service(root: &Path, bad_response: bool) -> Arc<dyn SessionStorage> {
    ModuleCatalog::selected_session_storage(&config(root, bad_response), root)
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn explicit_storage_service_supports_blocking_and_async_reads_without_other_modules() {
    let root = tempfile::tempdir().unwrap();
    let storage = service(root.path(), false);
    assert!(
        matches!(storage.execute_blocking(list(root.path())).unwrap(), SessionStorageResponse::Sessions { sessions } if sessions.is_empty())
    );
    assert!(
        matches!(storage.execute(list(root.path())).await.unwrap(), SessionStorageResponse::Sessions { sessions } if sessions.is_empty())
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("requests.log")).unwrap(),
        "execute\nexecute\n"
    );
    let invalid = SessionStorageRequest::ReadMetadata {
        session: crate::contracts::SessionLocator {
            session_id: crate::domain::new_session_id(),
            key: "fixture".into(),
        },
        key: "../outside".into(),
    };
    assert!(storage.execute_blocking(invalid).is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("requests.log")).unwrap(),
        "execute\nexecute\n"
    );
}

#[test]
fn storage_response_cannot_claim_an_unrelated_operation_result() {
    let root = tempfile::tempdir().unwrap();
    let storage = service(root.path(), true);
    assert!(storage.execute_blocking(list(root.path())).is_err());
}
