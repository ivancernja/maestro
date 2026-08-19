use anyhow::{Context, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// A real pty rendered by tui-term. Two uses: attaching a workspace's agent
/// session, and running a tool such as lazygit over the worktree. Keystrokes go
/// straight into the pty, so programs get full-fidelity input.
pub struct PtyHost {
    /// Identity for the view currently on screen; for an agent this is its
    /// tmux session name.
    pub label: String,
    /// Set only for tmux attachments, which must detach before teardown.
    detach_session: Option<String>,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    pub rows: u16,
    pub cols: u16,
    alive: Arc<AtomicBool>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl PtyHost {
    pub fn attach(session: &str, rows: u16, cols: u16, dirty: Arc<AtomicBool>) -> Result<Self> {
        let mut cmd = CommandBuilder::new("tmux");
        cmd.args(["attach", "-t", session]);
        // A nested client refuses to start while TMUX is set.
        cmd.env_remove("TMUX");
        Self::spawn(
            session.to_string(),
            Some(session.to_string()),
            cmd,
            rows,
            cols,
            dirty,
        )
    }

    /// Run a program over a directory, shown in place of the agent until it exits.
    pub fn run(
        label: &str,
        argv: &[String],
        cwd: &str,
        rows: u16,
        cols: u16,
        dirty: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (program, rest) = argv.split_first().context("empty command")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(rest);
        cmd.cwd(cwd);
        Self::spawn(label.to_string(), None, cmd, rows, cols, dirty)
    }

    fn spawn(
        label: String,
        detach_session: Option<String>,
        cmd: CommandBuilder,
        rows: u16,
        cols: u16,
        dirty: Arc<AtomicBool>,
    ) -> Result<Self> {
        let rows = rows.max(4);
        let cols = cols.max(20);

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening a pty")?;

        let mut child = pair.slave.spawn_command(cmd).context("spawning")?;
        drop(pair.slave);
        let killer = child.clone_killer();

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000)));
        let alive = Arc::new(AtomicBool::new(true));

        {
            let parser = parser.clone();
            let alive = alive.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut p) = parser.lock() {
                                p.process(&buf[..n]);
                            }
                            dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
                alive.store(false, Ordering::Relaxed);
                dirty.store(true, Ordering::Relaxed);
            });
        }
        thread::spawn(move || {
            let _ = child.wait();
        });

        Ok(Self {
            label,
            detach_session,
            parser,
            writer,
            master: pair.master,
            rows,
            cols,
            alive,
            killer,
        })
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(4);
        let cols = cols.max(20);
        if rows == self.rows && cols == self.cols {
            return;
        }
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
        self.rows = rows;
        self.cols = cols;
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn parser(&self) -> Option<MutexGuard<'_, vt100::Parser>> {
        self.parser.lock().ok()
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

impl Drop for PtyHost {
    fn drop(&mut self) {
        // Detach the client before tearing the pty down. Dropping the master on
        // its own leaves `tmux attach` alive, because the reader thread still
        // holds a duplicate of the master fd, and the agent's pane goes with it.
        if let Some(session) = &self.detach_session {
            let _ = std::process::Command::new("tmux")
                .args(["detach-client", "-s", session])
                .output();
        }
        let _ = self.killer.kill();
    }
}
