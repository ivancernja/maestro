use std::{
    collections::HashMap,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

/// One listening socket, read from `ss` and /proc. Self-contained on purpose:
/// the panel has to work on any machine, not only one with a particular helper
/// script installed.
#[derive(Debug, Clone)]
pub struct Port {
    pub port: u32,
    pub pid: u32,
    pub command: String,
    pub project: Option<String>,
    pub kind: String,
    /// A dev server rather than infrastructure, so it can lead the list.
    pub dev: bool,
    pub uptime: String,
    pub ports_held: u32,
}

/// Commands worth calling a dev server. Matched against the whole command line,
/// because "node" on its own says nothing.
const DEV_PATTERNS: [(&str, &str); 22] = [
    ("vite", "vite"),
    ("next dev", "next"),
    ("next-server", "next"),
    ("astro dev", "astro"),
    ("nuxt", "nuxt"),
    ("remix vite:dev", "remix"),
    ("react-scripts", "cra"),
    ("webpack", "webpack"),
    ("encore run", "encore"),
    ("encore daemon", "encore"),
    ("go run", "go"),
    ("uvicorn", "uvicorn"),
    ("flask run", "flask"),
    ("manage.py runserver", "django"),
    ("rails server", "rails"),
    ("php -S", "php"),
    ("http.server", "python"),
    ("storybook", "storybook"),
    ("jekyll", "jekyll"),
    ("hugo server", "hugo"),
    ("cargo watch", "cargo"),
    ("trunk serve", "trunk"),
];

/// Script runners report the script, not the framework behind it.
const RUNNERS: [&str; 5] = [
    "npm run dev",
    "pnpm dev",
    "yarn dev",
    "bun dev",
    "npm start",
];

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn cmdline(pid: u32) -> String {
    read(&format!("/proc/{pid}/cmdline"))
        .split('\0')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The git root's name if there is one, else the directory it was started in.
/// Memoised by directory: the panel refreshes while open, and this would
/// otherwise spawn a git process per listener per second.
fn project(pid: u32) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let key = cwd.to_string_lossy().into_owned();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock()
        && let Some(hit) = map.get(&key)
    {
        return hit.clone();
    }

    let resolved = Command::new("git")
        .args(["-C", &key, "rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
            std::path::Path::new(&root)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().into_owned()));

    if let Ok(mut map) = cache.lock() {
        map.insert(key, resolved.clone());
    }
    resolved
}

/// Seconds since the process started. Field 22 of /proc/<pid>/stat is in clock
/// ticks since boot, 100 per second on Linux.
fn uptime(pid: u32) -> String {
    let boot: f64 = read("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    let stat = read(&format!("/proc/{pid}/stat"));
    // The command field can hold spaces and parentheses, so fields are counted
    // from after the last ')'.
    let Some((_, rest)) = stat.rsplit_once(") ") else {
        return "?".into();
    };
    let started: f64 = rest
        .split_whitespace()
        .nth(19)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    let secs = (boot - started / 100.0).max(0.0) as u64;

    if secs < 90 {
        format!("{secs}s")
    } else if secs < 5400 {
        format!("{}m", secs / 60)
    } else if secs < 172_800 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn classify(command: &str, name: &str) -> (String, bool) {
    let lowered = command.to_lowercase();
    for (needle, label) in DEV_PATTERNS {
        if lowered.contains(needle) {
            return (label.to_string(), true);
        }
    }
    for runner in RUNNERS {
        if lowered.contains(runner) {
            let first = runner.split(' ').next().unwrap_or(runner);
            return (first.to_string(), true);
        }
    }
    let short = name.split('-').next().unwrap_or(name);
    (
        if short.is_empty() {
            name.to_string()
        } else {
            short.to_string()
        },
        false,
    )
}

/// Dev servers first, then by port. Rows without a pid are left out: `ss`
/// withholds it for other users, and a port that cannot be signalled is not
/// ours to offer.
pub fn list() -> Result<Vec<Port>, String> {
    let out = Command::new("ss")
        .args(["-tlnpH"])
        .output()
        .map_err(|e| format!("ss: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }

    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut found: HashMap<u32, Port> = HashMap::new();

    for line in text.lines() {
        // A process name can contain spaces, so the users field is found in the
        // raw line rather than by splitting on whitespace.
        let Some(users) = line.split("users:((").nth(1) else {
            continue;
        };
        let Some(pid) = users.split("pid=").nth(1).and_then(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .ok()
        }) else {
            continue;
        };
        let name = users
            .strip_prefix('"')
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("?")
            .to_string();

        let Some(local) = line.split_whitespace().nth(3) else {
            continue;
        };
        let Some((_, port)) = local.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.parse::<u32>() else {
            continue;
        };

        // One process can hold several ports; the panel wants a row for each.
        found.entry(port).or_insert_with(|| {
            let command = cmdline(pid);
            let (kind, dev) = classify(&command, &name);
            Port {
                port,
                pid,
                project: project(pid),
                kind,
                dev,
                uptime: uptime(pid),
                command,
                ports_held: 1,
            }
        });
    }

    let mut held: HashMap<u32, u32> = HashMap::new();
    for entry in found.values() {
        *held.entry(entry.pid).or_insert(0) += 1;
    }
    let mut ports: Vec<Port> = found.into_values().collect();
    for entry in ports.iter_mut() {
        entry.ports_held = held.get(&entry.pid).copied().unwrap_or(1);
    }
    ports.sort_by_key(|entry| (!entry.dev, entry.port));
    Ok(ports)
}

/// SIGTERM, then SIGKILL if it is still there. Spawned rather than waited on so
/// the ui never freezes for the grace period; the list refreshing is the
/// confirmation.
pub fn kill(entry: &Port) {
    let pid = entry.pid;
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "kill -TERM {pid} 2>/dev/null; \
             for _ in 1 2 3 4 5 6; do sleep 0.5; \
             kill -0 {pid} 2>/dev/null || exit 0; done; \
             kill -KILL {pid} 2>/dev/null"
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
