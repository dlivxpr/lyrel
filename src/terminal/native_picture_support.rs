// Agent-neutral helpers for the pinned native picture fixtures. The Pi, Kimi
// and Hermes consumers share the production scope encoder, the synthetic
// raster identity and the production paint observation; each fixture keeps
// only its own transcript witnesses.

/// Exercise the same production command encoder as launch/resume; no fixture
/// writes terminal identity or forces the agent's image policy itself.
fn native_graphics_scoped_shell(
    agent: crate::core::cli_agent::CLIAgent,
    mut shell: crate::daemon::protocol::ShellSpec,
) -> crate::daemon::protocol::ShellSpec {
    let program = if cfg!(windows) { "pwsh" } else { "sh" };
    let line = std::iter::once(shell.program.clone())
        .chain(shell.args.iter().cloned())
        .map(|arg| lyrel_core::core::shell_quote::quote_for_shell(&arg, Some(program)))
        .collect::<Vec<_>>()
        .join(" ");
    let host = lyrel_core::host::local::LocalHost::new();
    let line = if agent == crate::core::cli_agent::CLIAgent::Hermes {
        // Hermes' presence-sensitive marker belongs to the production agent
        // encoder, never a fixture-authored child environment override.
        lyrel_core::host::session_operations::scoped_agent_launch_command(
            &*host,
            agent,
            &line,
            Some(program),
        )
        .unwrap()
    } else {
        let line = lyrel_core::host::session_operations::agent_compatibility_command(agent, &line);
        lyrel_core::host::session_operations::scoped_launch_command(&*host, &line, Some(program))
            .unwrap()
    };
    let line = if cfg!(windows) {
        format!("Write-Output ('LYREL_PARENT_BEFORE=' + $env:TERM_PROGRAM); {line}; Write-Output ('LYREL_PARENT_AFTER=' + $env:TERM_PROGRAM)")
    } else {
        format!("printf 'LYREL_PARENT_BEFORE=%s\\n' \"$TERM_PROGRAM\"; {line}; printf 'LYREL_PARENT_AFTER=%s\\n' \"$TERM_PROGRAM\"")
    };
    shell.program = program.into();
    shell.args = if cfg!(windows) {
        vec!["-NoProfile".into(), "-Command".into(), line]
    } else {
        vec!["-c".into(), line]
    };
    shell
}

#[track_caller]
fn native_picture_wait(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    description: &str,
    ready: impl Fn(&TerminalView) -> bool,
) {
    graphics_recovery_wait_with(
        cx,
        std::time::Duration::from_secs(45),
        |cx| view.read_with(cx, |view, _| ready(view)),
        |cx| {
            view.read_with(cx, |view, _| {
                format!(
                    "native picture phase {description} timed out: child_exited={} disconnected={} text={:?} graphics={}",
                    view.terminal.child_exited(),
                    view.terminal.exited,
                    view.scrollback_text(),
                    view.terminal.images().recovery_diagnostics()
                )
            })
        },
    );
}

const NATIVE_PICTURE_FIRST: [[u8; 4]; 5] = [
    [0, 0, 255, 255],
    [0, 255, 0, 255],
    [255, 0, 0, 255],
    [0, 255, 255, 255],
    [255, 255, 255, 255],
];
const NATIVE_PICTURE_LAST: [[u8; 4]; 5] = [
    [11, 0, 255, 255],
    [7, 255, 0, 255],
    [255, 0, 5, 255],
    [0, 255, 255, 255],
    [255, 255, 255, 255],
];

fn native_picture_pixels(
    view: &TerminalView,
    expected: [[u8; 4]; 5],
) -> Option<crate::terminal::images::RenderPlacement> {
    graphics_visible_images(view).into_iter().find(|image| {
        if image.evicted {
            return false;
        }
        let Some(data) = &image.data else {
            return false;
        };
        let Some(bytes) = data.as_bytes(0) else {
            return false;
        };
        if bytes.len() != 48 * 24 * 4 {
            return false;
        }
        // Compare the complete raster, not just dominant color or landmarks.
        bytes.chunks_exact(4).enumerate().all(|(index, pixel)| {
            let (x, y) = (index % 48, index / 48);
            let quadrant = if (22..26).contains(&x) && (10..14).contains(&y) {
                4
            } else if y < 12 {
                usize::from(x >= 24)
            } else {
                2 + usize::from(x >= 24)
            };
            pixel == expected[quadrant]
        })
    })
}
