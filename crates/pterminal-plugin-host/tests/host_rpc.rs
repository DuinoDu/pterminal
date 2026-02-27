use pterminal_plugin_host::{
    HostRequest, HostRequestPayload, HostResponsePayload, PluginHostRuntime,
};

#[test]
fn typed_rpc_message_roundtrips_as_json() {
    let req = HostRequest {
        id: 7,
        payload: HostRequestPayload::Handshake {
            protocol_version: "1.0".into(),
            host_capabilities: vec!["command.execute".into()],
        },
    };
    let raw = serde_json::to_string(&req).expect("serialize");
    let decoded: HostRequest = serde_json::from_str(&raw).expect("deserialize");
    assert_eq!(decoded, req);
}

#[test]
fn runtime_handles_activate_list_and_deactivate() {
    let mut runtime = PluginHostRuntime::new(vec!["command.execute".into()]);

    let activated = runtime.handle(HostRequest {
        id: 1,
        payload: HostRequestPayload::Activate {
            plugin_id: "acme.workspace-sidebar".into(),
        },
    });
    assert_eq!(
        activated.payload,
        HostResponsePayload::Activated {
            plugin_id: "acme.workspace-sidebar".into()
        }
    );

    let listed = runtime.handle(HostRequest {
        id: 2,
        payload: HostRequestPayload::ListActivePlugins,
    });
    assert_eq!(
        listed.payload,
        HostResponsePayload::ActivePlugins {
            plugin_ids: vec!["acme.workspace-sidebar".into()]
        }
    );

    let deactivated = runtime.handle(HostRequest {
        id: 3,
        payload: HostRequestPayload::Deactivate {
            plugin_id: "acme.workspace-sidebar".into(),
        },
    });
    assert_eq!(
        deactivated.payload,
        HostResponsePayload::Deactivated {
            plugin_id: "acme.workspace-sidebar".into()
        }
    );
}

#[test]
fn json_line_dispatch_reports_decode_errors() {
    let mut runtime = PluginHostRuntime::new(vec![]);
    let err = runtime
        .handle_json_line("{not-json")
        .expect_err("invalid json should fail");
    assert!(err.to_string().contains("failed to decode"));
}
