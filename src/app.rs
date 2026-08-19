use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::{
    collections::HashMap,
    collections::HashSet,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use crate::{
    agent, git, keys, names,
    ports::{self, Port},
    registry::{self, Workspace},
    term::PtyHost,
    theme::Theme,
    tmux,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Working,
    Idle,
    Gone,
}

pub struct Row {
    pub ws: Workspace,
    pub status: Status,
    /// Went quiet while you were looking elsewhere.
    pub unseen: bool,
    pub added: u32,
    pub removed: u32,
    pub since: u64,
    pub age: u64,
}

pub enum Step {
    Repo,
    Branch,
    Agent,
    Task,
}

pub struct NewForm {
    pub step: Step,
    /// Where to go back to if the form is abandoned.
    from_insert: bool,
    /// A suggested name is replaced wholesale by the first keystroke, the way
    /// selected text would be, rather than typed into.
    pub branch_is_suggestion: bool,
    root: PathBuf,
    pub repos: Vec<PathBuf>,
    pub repo_sel: usize,
    pub branch: String,
    pub agents: Vec<String>,
    pub agent_sel: usize,
    pub task: String,
}

/// Where to look for repositories, with the depth to search each. Honours
/// MAESTRO_REPO_ROOT first (colon-separated for several), then the directories
/// people actually keep code in, then $HOME shallowly so a fresh install still
/// finds something.
fn repo_roots() -> Vec<(PathBuf, usize)> {
    if let Ok(raw) = std::env::var("MAESTRO_REPO_ROOT") {
        let listed: Vec<(PathBuf, usize)> = raw
            .split(':')
            .filter(|part| !part.is_empty())
            .map(|part| (PathBuf::from(part), 3))
            .collect();
        if !listed.is_empty() {
            return listed;
        }
    }

    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    let conventional: Vec<(PathBuf, usize)> = [
        "Work",
        "work",
        "code",
        "Code",
        "src",
        "dev",
        "Developer",
        "projects",
        "Projects",
        "repos",
        "git",
    ]
    .iter()
    .map(|name| home.join(name))
    .filter(|path| path.is_dir())
    .map(|path| (path, 3))
    .collect();

    if conventional.is_empty() {
        vec![(home, 2)]
    } else {
        conventional
    }
}

impl NewForm {
    fn new(from_insert: bool) -> Self {
        let mut repos: Vec<PathBuf> = Vec::new();
        for root in repo_roots() {
            repos.extend(git::find_repos(&root.0, root.1));
        }
        repos.sort();
        repos.dedup();
        let agents = {
            let installed = agent::installed();
            if installed.is_empty() {
                vec!["claude".to_string()]
            } else {
                installed
            }
        };
        let default = agent::default_agent();
        let agent_sel = agents.iter().position(|a| *a == default).unwrap_or(0);
        Self {
            step: Step::Repo,
            from_insert,
            branch_is_suggestion: false,
            root: PathBuf::new(),
            repos,
            repo_sel: 0,
            branch: String::new(),
            agents,
            agent_sel,
            task: String::new(),
        }
    }
}

pub enum Mode {
    Normal,
    Insert,
    New(NewForm),
    Confirm,
    Land,
    Ports,
}

pub struct App {
    pub rows: Vec<Row>,
    pub selected: usize,
    pub mode: Mode,
    pub focused: Option<String>,
    pub pty: Option<PtyHost>,
    pub dirty: Arc<AtomicBool>,
    pub quit: bool,
    pub message: Option<String>,
    pub main_area: Rect,
    pub sidebar_area: Rect,
    pub sidebar_offset: usize,
    /// Row of the first workspace entry, which sits below a blank spacer line.
    pub sidebar_top: u16,
    pub theme: Theme,
    /// A tool run over the worktree — lazygit, a shell, an editor — shown in
    /// place of the agent until it exits.
    pub overlay: Option<PtyHost>,
    pub overlay_title: String,
    pub ports: Vec<Port>,
    pub port_sel: usize,
    /// Kills escalate to SIGKILL, so one is confirmed before it is sent.
    pub port_pending: Option<u32>,
    tracker: HashMap<String, (u64, Instant)>,
    last_status: HashMap<String, Status>,
    unseen: HashSet<String>,
}

impl App {
    pub fn new(dirty: Arc<AtomicBool>) -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            mode: Mode::Normal,
            focused: None,
            pty: None,
            dirty,
            quit: false,
            message: None,
            main_area: Rect::new(0, 0, 80, 24),
            sidebar_area: Rect::new(0, 0, 0, 0),
            sidebar_offset: 0,
            sidebar_top: 0,
            theme: Theme::load(),
            overlay: None,
            overlay_title: String::new(),
            ports: Vec::new(),
            port_sel: 0,
            port_pending: None,
            tracker: HashMap::new(),
            last_status: HashMap::new(),
            unseen: HashSet::new(),
        }
    }

    /// Whether an agent is doing something is read from whether its screen
    /// changed since the last poll: a moving spinner is work, a still screen is
    /// not. That cannot yet tell "waiting for you" from "finished".
    fn probe(&mut self, ws: &Workspace) -> Status {
        if !tmux::has_session(&ws.session) || tmux::pane_dead(&ws.session) {
            self.tracker.remove(&ws.id);
            return Status::Gone;
        }
        let Some(pane) = tmux::first_pane(&ws.session) else {
            return Status::Gone;
        };
        let content = tmux::capture(&pane).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let hash = hasher.finish();

        match self.tracker.get(&ws.id) {
            Some((prev, at)) if *prev == hash => {
                if at.elapsed().as_secs() >= 3 {
                    Status::Idle
                } else {
                    Status::Working
                }
            }
            _ => {
                self.tracker.insert(ws.id.clone(), (hash, Instant::now()));
                Status::Working
            }
        }
    }

    pub fn refresh(&mut self) {
        // Picks up `omarchy theme set` while the app is running.
        if self.theme.is_stale() {
            self.theme = Theme::load();
        }

        if matches!(self.mode, Mode::Ports) {
            self.load_ports();
        }

        let now = registry::now();
        let watching = self.focused.clone();
        let mut rows = Vec::new();
        for ws in registry::load_all() {
            let status = self.probe(&ws);

            // An agent that goes quiet while you are looking elsewhere is the
            // only event worth announcing. Looking at it is what clears it.
            let previous = self.last_status.get(&ws.id).copied();
            let looking = watching.as_deref() == Some(ws.id.as_str());
            if looking {
                self.unseen.remove(&ws.id);
            } else if previous == Some(Status::Working)
                && status == Status::Idle
                && self.unseen.insert(ws.id.clone())
            {
                notify(&ws);
            }
            self.last_status.insert(ws.id.clone(), status);
            let (added, removed) = git::diffstat(&PathBuf::from(&ws.worktree));
            let since = self
                .tracker
                .get(&ws.id)
                .map(|(_, at)| at.elapsed().as_secs())
                .unwrap_or(0);
            rows.push(Row {
                age: now.saturating_sub(ws.created_at),
                unseen: self.unseen.contains(&ws.id),
                ws,
                status,
                added,
                removed,
                since,
            });
        }
        self.rows = rows;

        if self.rows.is_empty() {
            self.selected = 0;
            self.focused = None;
            self.pty = None;
            return;
        }
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
        // The main pane always shows the selected workspace.
        let want = self.rows[self.selected].ws.id.clone();
        if self.focused.as_deref() != Some(want.as_str()) {
            self.focused = Some(want);
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    fn move_sel(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as i32;
        let next = (self.selected as i32 + delta).clamp(0, len - 1);
        if next as usize != self.selected {
            self.selected = next as usize;
            let id = self.rows[self.selected].ws.id.clone();
            self.unseen.remove(&id);
            self.rows[self.selected].unseen = false;
            self.focused = Some(id);
        }
    }

    /// Attaches, resizes, or drops the pty so it always matches the selection
    /// and the current layout.
    pub fn sync_pty(&mut self) {
        if let Some(overlay) = self.overlay.as_mut() {
            if overlay.is_alive() {
                overlay.resize(self.main_area.height, self.main_area.width);
                return;
            }
            self.overlay = None;
            self.overlay_title.clear();
            self.mode = Mode::Normal;
        }

        let Some(id) = self.focused.clone() else {
            self.pty = None;
            return;
        };
        let Some(row) = self.rows.iter().find(|r| r.ws.id == id) else {
            self.pty = None;
            return;
        };
        // Attach whenever the session is still there, even for a finished agent:
        // its last screen is usually why you are looking.
        let want = row.ws.session.clone();
        if !tmux::has_session(&want) {
            self.pty = None;
            return;
        }
        let (rows, cols) = (self.main_area.height, self.main_area.width);
        let stale = match &self.pty {
            Some(p) => p.label != want || !p.is_alive(),
            None => true,
        };

        if stale {
            match PtyHost::attach(&want, rows, cols, self.dirty.clone()) {
                Ok(p) => self.pty = Some(p),
                Err(e) => {
                    self.pty = None;
                    self.message = Some(format!("could not attach: {e}"));
                }
            }
        } else if let Some(p) = self.pty.as_mut() {
            p.resize(rows, cols);
        }
    }

    fn open_overlay(&mut self, title: &str, argv: Vec<String>) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let cwd = row.ws.worktree.clone();
        match PtyHost::run(
            title,
            &argv,
            &cwd,
            self.main_area.height,
            self.main_area.width,
            self.dirty.clone(),
        ) {
            Ok(pty) => {
                self.overlay = Some(pty);
                self.overlay_title = title.to_string();
                self.message = None;
                // The tool needs the keyboard immediately.
                self.mode = Mode::Insert;
            }
            Err(e) => self.message = Some(format!("{title}: {e}")),
        }
    }

    fn editor_argv() -> Vec<String> {
        // $EDITOR may carry arguments, as omarchy's own does.
        let raw = std::env::var("EDITOR").unwrap_or_else(|_| "nvim".into());
        let argv: Vec<String> = raw.split_whitespace().map(|s| s.to_string()).collect();
        if argv.is_empty() {
            vec!["nvim".into()]
        } else {
            argv
        }
    }

    /// Relaunch an agent in its existing worktree, so a crash does not force you
    /// to destroy the branch to recover.
    fn restart_agent(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let ws = row.ws.clone();
        if tmux::has_session(&ws.session) && !tmux::pane_dead(&ws.session) {
            self.message = Some(format!("{} is still running", ws.branch));
            return;
        }

        tmux::kill_session(&ws.session);
        let command = std::env::var("MAESTRO_AGENT_CMD")
            .unwrap_or_else(|_| agent::command(&ws.agent).unwrap_or("bash").to_string());

        match tmux::new_session(&ws.session, &ws.worktree, &command) {
            Ok(()) => {
                self.pty = None;
                self.message = None;
                self.last_status.remove(&ws.id);
                self.refresh();
            }
            Err(e) => self.message = Some(format!("could not restart: {e}")),
        }
    }

    pub fn land_target(&self) -> Option<(String, String)> {
        let row = self.rows.get(self.selected)?;
        let root = PathBuf::from(&row.ws.repo_path);
        let base = if row.ws.base.is_empty() {
            git::current_branch(&root).unwrap_or_else(|| "main".into())
        } else {
            row.ws.base.clone()
        };
        Some((row.ws.branch.clone(), base))
    }

    /// Merge the workspace branch into the branch it came from, then retire the
    /// workspace. Every precondition is checked first and reported by name:
    /// this touches the repo you actually work in.
    fn land(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let ws = row.ws.clone();
        let root = PathBuf::from(&ws.repo_path);
        let worktree = PathBuf::from(&ws.worktree);
        let base = if ws.base.is_empty() {
            git::current_branch(&root).unwrap_or_else(|| "main".into())
        } else {
            ws.base.clone()
        };

        if !git::is_clean(&worktree) {
            self.message = Some(format!(
                "{} has uncommitted changes — commit them with g first",
                ws.branch
            ));
            return;
        }
        if git::ahead_of(&root, &base, &ws.branch) == 0 {
            self.message = Some(format!("{} has no commits beyond {base}", ws.branch));
            return;
        }
        match git::current_branch(&root) {
            Some(current) if current == base => {}
            Some(current) => {
                self.message = Some(format!("{} is on {current}, not {base}", ws.repo));
                return;
            }
            None => {
                self.message = Some(format!("{} is not on a branch", ws.repo));
                return;
            }
        }
        if !git::is_clean(&root) {
            self.message = Some(format!("{} has uncommitted changes", ws.repo));
            return;
        }

        match git::merge(&root, &ws.branch) {
            Ok(_) => {
                self.delete_selected();
                self.message = Some(format!("merged {} into {base}", ws.branch));
            }
            Err(e) => self.message = Some(format!("merge failed: {e}")),
        }
    }

    pub fn create(
        &mut self,
        repo: &Path,
        branch: &str,
        agent_name: &str,
        task: &str,
    ) -> Result<String> {
        let root = git::repo_root(repo)?;
        let repo_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into());
        let id = format!("{repo_name}--{branch}");

        if registry::exists(&id) {
            bail!("workspace {id} already exists");
        }
        let worktree = git::worktree_path(&root, branch);
        if worktree.exists() {
            bail!("{} already exists", worktree.display());
        }

        git::add_worktree(&root, branch, &worktree)?;
        let _ = std::process::Command::new("mise")
            .arg("trust")
            .arg(&worktree)
            .output();

        let command = std::env::var("MAESTRO_AGENT_CMD").unwrap_or_else(|_| {
            agent::command_with_prompt(agent_name, task).unwrap_or_else(|| "bash".to_string())
        });
        let session = tmux::session_for(&id);
        tmux::new_session(&session, &worktree.to_string_lossy(), &command)?;

        registry::save(&Workspace {
            id: id.clone(),
            branch: branch.to_string(),
            base: git::current_branch(&root).unwrap_or_default(),
            repo: repo_name,
            repo_path: root.to_string_lossy().into_owned(),
            worktree: worktree.to_string_lossy().into_owned(),
            agent: agent_name.to_string(),
            session,
            created_at: registry::now(),
        })?;

        Ok(id)
    }

    fn delete_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let ws = row.ws.clone();
        tmux::kill_session(&ws.session);
        let root = PathBuf::from(&ws.repo_path);
        git::remove_worktree(&root, &PathBuf::from(&ws.worktree));
        git::delete_branch(&root, &ws.branch);
        let _ = registry::delete(&ws.id);
        self.tracker.remove(&ws.id);
        if self.focused.as_deref() == Some(ws.id.as_str()) {
            self.focused = None;
            self.pty = None;
        }
        self.message = Some(format!("removed {}", ws.id));
        self.refresh();
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match &mut self.mode {
            Mode::Insert => self.key_insert(key),
            Mode::Normal => self.key_normal(key),
            Mode::Confirm => self.key_confirm(key),
            Mode::Land => self.key_land(key),
            Mode::Ports => self.key_ports(key),
            Mode::New(_) => self.key_new(key),
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        // A modal owns the screen while it is open.
        if matches!(self.mode, Mode::New(_) | Mode::Confirm) {
            return;
        }

        let area = self.sidebar_area;
        let in_sidebar = ev.column >= area.x
            && ev.column < area.x.saturating_add(area.width)
            && ev.row >= area.y
            && ev.row < area.y.saturating_add(area.height);

        if in_sidebar {
            match ev.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    // Rows are three lines tall, measured from the first entry.
                    if ev.row < self.sidebar_top {
                        return;
                    }
                    let index = self.sidebar_offset + ((ev.row - self.sidebar_top) / 3) as usize;
                    if index < self.rows.len() {
                        self.selected = index;
                        self.focused = Some(self.rows[index].ws.id.clone());
                    }
                }
                MouseEventKind::ScrollDown => self.move_sel(1),
                MouseEventKind::ScrollUp => self.move_sel(-1),
                _ => {}
            }
            return;
        }

        self.forward_mouse(ev);
    }

    /// Mouse events over the agent belong to the agent, but only when it asked
    /// for them and speaks SGR; otherwise the bytes would be rendered as text.
    fn forward_mouse(&mut self, ev: MouseEvent) {
        let main = self.main_area;
        let Some(pty) = self.pty.as_mut() else {
            return;
        };

        let wanted = {
            let Some(parser) = pty.parser() else {
                return;
            };
            let screen = parser.screen();
            screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None
                && screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr
        };
        if !wanted {
            return;
        }

        let code = |button: MouseButton| match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        let (button, released) = match ev.kind {
            MouseEventKind::Down(b) => (code(b), false),
            MouseEventKind::Up(b) => (code(b), true),
            MouseEventKind::Drag(b) => (code(b) + 32, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            _ => return,
        };

        let column = ev.column.saturating_sub(main.x) + 1;
        let row = ev.row.saturating_sub(main.y) + 1;
        let final_byte = if released { 'm' } else { 'M' };
        pty.send(format!("\x1b[<{button};{column};{row}{final_byte}").as_bytes());
    }

    fn key_insert(&mut self, key: KeyEvent) {
        // Ctrl+Q is maestro's; everything else belongs to the agent.
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.overlay = None;
            self.overlay_title.clear();
            self.mode = Mode::Normal;
            return;
        }

        // While a tool is up it owns every key, including the alt chords: moving
        // workspaces under an open lazygit would be nonsense.
        if let Some(overlay) = self.overlay.as_mut() {
            let bytes = keys::encode(&key);
            if !bytes.is_empty() {
                overlay.send(&bytes);
            }
            return;
        }

        // A few keys stay maestro's while typing, so starting a workspace or
        // moving between agents never needs a detour through normal mode.
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char('n') => {
                    self.message = None;
                    self.mode = Mode::New(NewForm::new(true));
                    return;
                }
                KeyCode::Char('j') => {
                    self.move_sel(1);
                    return;
                }
                KeyCode::Char('k') => {
                    self.move_sel(-1);
                    return;
                }
                _ => {}
            }
        }

        let bytes = keys::encode(&key);
        if bytes.is_empty() {
            return;
        }
        if let Some(pty) = self.pty.as_mut() {
            pty.send(&bytes);
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Home => {
                self.selected = 0;
                self.refresh();
            }
            KeyCode::End => {
                if !self.rows.is_empty() {
                    self.selected = self.rows.len() - 1;
                    self.refresh();
                }
            }
            KeyCode::Enter | KeyCode::Char('i') => {
                if self.pty.is_some() {
                    self.mode = Mode::Insert;
                }
            }
            KeyCode::Char('n') => {
                self.message = None;
                self.mode = Mode::New(NewForm::new(false));
            }
            KeyCode::Char('d') => {
                if !self.rows.is_empty() {
                    self.mode = Mode::Confirm;
                }
            }
            KeyCode::Char('r') => {
                self.message = None;
                self.refresh();
            }
            KeyCode::Char('g') => self.open_overlay("lazygit", vec!["lazygit".into()]),
            KeyCode::Char('s') => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
                self.open_overlay("shell", vec![shell]);
            }
            KeyCode::Char('e') => self.open_overlay("editor", Self::editor_argv()),
            KeyCode::Char('R') => self.restart_agent(),
            KeyCode::Char('p') => {
                self.message = None;
                self.port_pending = None;
                self.port_sel = 0;
                self.load_ports();
                self.mode = Mode::Ports;
            }
            KeyCode::Char('L') => {
                if !self.rows.is_empty() {
                    self.mode = Mode::Land;
                }
            }
            // Errors persist until dismissed; clearing them on the next keypress
            // made failures flash past unread.
            KeyCode::Esc => self.message = None,
            _ => {}
        }
    }

    pub fn load_ports(&mut self) {
        match ports::list() {
            Ok(list) => {
                self.ports = list;
                if self.port_sel >= self.ports.len() {
                    self.port_sel = self.ports.len().saturating_sub(1);
                }
            }
            Err(e) => {
                self.ports.clear();
                self.message = Some(e);
            }
        }
    }

    fn key_ports(&mut self, key: KeyEvent) {
        // A pending kill takes the next key, so y/n never leaks into navigation.
        if let Some(port) = self.port_pending {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    if let Some(entry) = self.ports.iter().find(|p| p.port == port) {
                        let label = entry.kind.clone();
                        ports::kill(entry);
                        self.message = Some(format!("stopping {label} on {port}"));
                    }
                }
                _ => {}
            }
            self.port_pending = None;
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('p') | KeyCode::Char('q') => {
                self.message = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.ports.is_empty() {
                    self.port_sel = (self.port_sel + 1).min(self.ports.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.port_sel = self.port_sel.saturating_sub(1);
            }
            KeyCode::Char('r') => {
                self.message = None;
                self.load_ports();
            }
            KeyCode::Char('x') | KeyCode::Enter => {
                if let Some(entry) = self.ports.get(self.port_sel) {
                    // Cleared so the confirm prompt is what the footer shows.
                    self.message = None;
                    self.port_pending = Some(entry.port);
                }
            }
            _ => {}
        }
    }

    fn key_land(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.land();
            }
            _ => self.mode = Mode::Normal,
        }
    }

    fn key_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.delete_selected();
            }
            _ => self.mode = Mode::Normal,
        }
    }

    fn key_new(&mut self, key: KeyEvent) {
        let Mode::New(form) = &mut self.mode else {
            return;
        };

        if key.code == KeyCode::Esc {
            let from_insert = form.from_insert;
            match form.step {
                // Abandoning the form returns you where you opened it from.
                Step::Repo => {
                    self.mode = if from_insert {
                        Mode::Insert
                    } else {
                        Mode::Normal
                    }
                }
                Step::Branch => form.step = Step::Repo,
                Step::Agent => form.step = Step::Branch,
                Step::Task => form.step = Step::Agent,
            }
            return;
        }

        match form.step {
            Step::Repo => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if !form.repos.is_empty() {
                        form.repo_sel = (form.repo_sel + 1).min(form.repos.len() - 1);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    form.repo_sel = form.repo_sel.saturating_sub(1);
                }
                KeyCode::Enter if !form.repos.is_empty() => {
                    let repo = form.repos[form.repo_sel].clone();
                    form.root = git::repo_root(&repo).unwrap_or(repo);
                    form.branch = names::suggest(&form.root);
                    form.branch_is_suggestion = true;
                    form.step = Step::Branch;
                }
                _ => {}
            },
            Step::Branch => match key.code {
                KeyCode::Char(c) => {
                    if form.branch_is_suggestion {
                        form.branch.clear();
                        form.branch_is_suggestion = false;
                    }
                    form.branch.push(c);
                }
                KeyCode::Tab => {
                    form.branch = names::suggest(&form.root);
                    form.branch_is_suggestion = true;
                }
                KeyCode::Backspace => {
                    if form.branch_is_suggestion {
                        form.branch.clear();
                        form.branch_is_suggestion = false;
                    } else {
                        form.branch.pop();
                    }
                }
                KeyCode::Enter if !form.branch.trim().is_empty() => {
                    form.step = Step::Agent;
                }
                _ => {}
            },
            Step::Agent => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    form.agent_sel = (form.agent_sel + 1).min(form.agents.len() - 1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    form.agent_sel = form.agent_sel.saturating_sub(1);
                }
                KeyCode::Enter => form.step = Step::Task,
                _ => {}
            },
            Step::Task => match key.code {
                KeyCode::Char(c) => form.task.push(c),
                KeyCode::Backspace => {
                    form.task.pop();
                }
                KeyCode::Enter => {
                    let repo = form.repos[form.repo_sel].clone();
                    let branch = form.branch.trim().to_string();
                    let agent_name = form.agents[form.agent_sel].clone();
                    let task = form.task.trim().to_string();
                    self.mode = Mode::Normal;
                    match self.create(&repo, &branch, &agent_name, &task) {
                        Ok(id) => {
                            self.refresh();
                            if let Some(i) = self.rows.iter().position(|r| r.ws.id == id) {
                                self.selected = i;
                                self.focused = Some(id);
                            }
                        }
                        Err(e) => self.message = Some(e.to_string()),
                    }
                }
                _ => {}
            },
        }
    }
}

/// A desktop notification through omarchy's own sender, fired once when an agent
/// goes quiet unwatched. Spawned rather than waited on so the ui never stalls.
fn notify(ws: &Workspace) {
    let headline = format!("{} is waiting", ws.branch);
    let detail = format!("{} · {}", ws.repo, ws.agent);

    // omarchy's sender is themed and carries a glyph; notify-send is the
    // portable equivalent everywhere else.
    let omarchy = std::process::Command::new("omarchy-notification-send")
        .args(["-u", "low", "-g", "\u{f0ae7}", &headline, &detail])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if omarchy.is_ok() {
        return;
    }

    let _ = std::process::Command::new("notify-send")
        .args(["-u", "low", "-a", "maestro", &headline, &detail])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
