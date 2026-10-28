//! Negotiated connection lifetime; retirement follows the read loop, not Arc drop.
use std::io::{self, Read, Write};
use std::sync::Arc;

use crate::core::machine;
use crate::daemon::control::LinkShutdown;
use crate::daemon::duplex::{Duplex, Halves};
use crate::host::SharedHost;

use super::attachments::ConnectionIdentity;
use super::dispatch::read_loop;
use super::output::Sink;
use super::subscriptions::subscribe_machine;
use super::{Conn, GitStreams, Pool, Requests, Services, Watches, negotiation};

pub fn serve<D: Duplex>(link: D, host: SharedHost) -> io::Result<()> {
    serve_with(link, host, Services::none())
}

pub fn serve_with<D: Duplex>(link: D, host: SharedHost, services: Services) -> io::Result<()> {
    let label = link.kind_label();
    let Halves {
        read,
        write,
        shutdown,
    } = link.split()?;
    serve_halves_with(read, write, shutdown, host, services, label)
}

pub fn serve_halves<R, W>(
    r: R,
    w: W,
    shutdown: Arc<dyn LinkShutdown>,
    host: SharedHost,
    label: &'static str,
) -> io::Result<()>
where
    R: Read,
    W: Write + Send + 'static,
{
    serve_halves_with(r, w, shutdown, host, Services::none(), label)
}

pub fn serve_halves_with<R, W>(
    mut r: R,
    w: W,
    shutdown: Arc<dyn LinkShutdown>,
    host: SharedHost,
    services: Services,
    label: &'static str,
) -> io::Result<()>
where
    R: Read,
    W: Write + Send + 'static,
{
    let sink = Arc::new(Sink::new(w));

    let hello = match negotiation::handshake(&mut r, &sink, &*host, &services) {
        Ok(Some(hello)) => hello,
        Ok(None) => {
            let _ = shutdown.shutdown_link();
            return Ok(());
        }
        Err(e) => {
            let _ = shutdown.shutdown_link();
            return if is_disconnect(&e) {
                log::debug!("control peer ({label}) left during the handshake: {e}");
                Ok(())
            } else {
                Err(e)
            };
        }
    };

    let machine_sub = subscribe_machine(&services, &sink);

    let conn = Arc::new(Conn {
        host,
        sink: Arc::clone(&sink),
        requests: Requests::new(),
        watches: Watches::new(),
        git_streams: GitStreams::new(),
        pool: Pool::new(),
        machine: services.machine.clone(),
        machine_origin: machine_sub.as_ref().map(machine::Subscription::id),
        panes: services.panes.clone(),
        identity: ConnectionIdentity::new(
            &hello,
            &sink,
            &shutdown,
            Arc::clone(&services.attachments),
        ),
    });

    if hello.gui {
        // GUI registration starts only after a successful version handshake,
        // so an incompatible app can never be reported as a live receiver.
        conn.identity.register_gui();
    }

    if let Some(workspace) = hello.workspace.as_deref()
        && let Err(e) = conn
            .identity
            .attach(conn.machine.as_deref(), workspace, true)
    {
        log::warn!("control peer ({label}) could not attach to workspace {workspace}: {e}");
    }

    let outcome = read_loop(&mut r, &conn);

    // Workers may still own Conn. Closing scheduling and retiring the sink here
    // is intentional: last-Arc drop must not delay peer retirement or GUI release.

    conn.pool.close();
    conn.watches.clear();
    conn.identity.release_all(conn.machine.as_deref());
    // Every exit path converges here, including EOF and protocol errors.
    conn.identity.unregister_gui();
    drop(machine_sub);
    sink.retire();
    let _ = shutdown.shutdown_link();

    match outcome {
        Ok(()) => {
            log::debug!("control connection ({label}) closed by peer");
            Ok(())
        }
        Err(e) if is_disconnect(&e) => {
            log::debug!("control connection ({label}) ended: {e}");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod unix_tests;
