use super::*;
    #[cfg(unix)]
    #[test]
    fn a_descriptor_that_is_not_a_terminal_is_refused_rather_than_adopted() {
        use std::os::fd::AsRawFd as _;

        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        let err = AdoptedMaster::from_fd(file.as_raw_fd())
            .err()
            .expect("a plain file is not a pty");
        assert!(
            err.to_string().contains("not a terminal"),
            "the refusal was {err}"
        );
        // Refused means not taken: the caller still owns what it passed, and
        // dropping `file` here is what closes it. Adopting first and failing
        // later would have closed a descriptor belonging to someone else.
        drop(file);
    }

    #[cfg(unix)]
    #[test]
    fn an_adopted_master_goes_back_to_close_on_exec() {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("open a pty");
        let tty = pair
            .master
            .tty_name()
            .expect("a fresh pty has a slave name");
        let fd = pair
            .master
            .as_raw_fd()
            .expect("a fresh master has a descriptor");
        // What crossing the exec leaves behind: `dup` hands out a descriptor
        // with close-on-exec clear, exactly like one that was deliberately
        // stripped to survive.
        let inherited = unsafe { libc::dup(fd) };
        assert!(inherited >= 0, "dup the master");
        let master = AdoptedMaster::from_fd(inherited).expect("a pty is adoptable");
        assert_eq!(master.tty_name().as_deref(), Some(tty.as_path()));
        let flags = unsafe { libc::fcntl(inherited, libc::F_GETFD) };
        assert!(
            flags >= 0 && flags & libc::FD_CLOEXEC != 0,
            "an adopted master left inheritable ends up in every child this daemon spawns, \
             and a child holding it keeps the pty from ever hanging up"
        );
        drop(master);
    }
