use super::Stream;

pub(super) fn socket_pair() -> (Stream, Stream) {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::pair().unwrap()
    }
    #[cfg(windows)]
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client_side = std::net::TcpStream::connect(addr).unwrap();
        let (daemon_side, _) = listener.accept().unwrap();
        (client_side, daemon_side)
    }
}

#[cfg(unix)]
pub(super) fn ssh_workspace() -> super::PaneWorkspace {
    super::PaneWorkspace {
        workspace: crate::core::session::WorkspaceId::new(),
        target: crate::core::session::RemoteTarget::Direct {
            user: "me".into(),
            host: "build-box".into(),
            port: 22,
        },
        spec: Some(Box::new(
            serde_json::from_str(
                r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
            )
            .unwrap(),
        )),
        label: None,
        resize_echo: false,
    }
}
