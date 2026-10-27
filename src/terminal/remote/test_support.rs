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
