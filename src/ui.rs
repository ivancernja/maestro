use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tui_term::widget::PseudoTerminal;

use crate::app::{App, Mode, Status, Step};
use crate::theme::Theme;

const SIDEBAR_WIDTH: u16 = 32;

fn dim(theme: &Theme) -> Style {
    Style::default().fg(theme.muted)
}

fn elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Exactly `width` columns: truncated with an ellipsis, or padded out, so
/// callers can lay out fixed columns by concatenation.
fn fit(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        if width == 0 {
            return String::new();
        }
        let mut out: String = chars[..width.saturating_sub(1)].iter().collect();
        out.push('…');
        out
    } else {
        format!("{s:<width$}")
    }
}

/// `~` for home, and truncated from the left so the meaningful tail survives.
fn short_path(path: &std::path::Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let text = path.to_string_lossy().into_owned();
    if !home.is_empty() && text.starts_with(&home) {
        format!("~{}", &text[home.len()..])
    } else {
        text
    }
}

fn tail(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width || width == 0 {
        return s.to_string();
    }
    let start = chars.len() - width.saturating_sub(1);
    format!("…{}", chars[start..].iter().collect::<String>())
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(20)]).areas(body);

    render_header(frame, app, header);
    render_sidebar(frame, app, sidebar);
    render_main(frame, app, main);
    render_footer(frame, app, footer);

    match &app.mode {
        Mode::New(_) => render_new(frame, app, frame.area()),
        Mode::Confirm => render_confirm(frame, app, frame.area()),
        Mode::Land => render_land(frame, app, frame.area()),
        Mode::Ports => render_ports(frame, app, frame.area()),
        _ => {}
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let working = app
        .rows
        .iter()
        .filter(|r| r.status == Status::Working)
        .count();
    let waiting = app
        .rows
        .iter()
        .filter(|r| r.status == Status::Idle && r.unseen)
        .count();
    let gone = app.rows.iter().filter(|r| r.status == Status::Gone).count();

    let mut spans = vec![
        Span::styled(
            " MAESTRO",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} workspace{}",
                app.rows.len(),
                if app.rows.len() == 1 { "" } else { "s" }
            ),
            dim(&t),
        ),
    ];
    if working > 0 {
        spans.push(Span::styled(
            format!("   ● {working}"),
            Style::default().fg(t.green),
        ));
    }
    if waiting > 0 {
        spans.push(Span::styled(
            format!("   ◆ {waiting} waiting"),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    }
    if gone > 0 {
        spans.push(Span::styled(
            format!("   ✗ {gone}"),
            Style::default().fg(t.red),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(dim(&t))
        .title(Span::styled(" workspaces ", dim(&t)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.sidebar_area = inner;

    if app.rows.is_empty() {
        app.sidebar_offset = 0;
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  no workspaces yet", dim(&t))),
            Line::from(""),
            Line::from(Span::styled("  press n to start one", dim(&t))),
        ]);
        frame.render_widget(empty, inner);
        return;
    }

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Scroll so the selection stays visible in a long list.
    let per_row = 3usize;
    // One blank line of breathing room under the title before the first entry.
    let visible = ((inner.height as usize).saturating_sub(1) / per_row).max(1);
    let offset = app.selected.saturating_sub(visible.saturating_sub(1));
    app.sidebar_offset = offset;
    app.sidebar_top = inner.y + 1;
    lines.push(Line::from(""));

    for (i, row) in app.rows.iter().enumerate().skip(offset) {
        let selected = i == app.selected;
        // Seen and quiet recedes; quiet with something new wears the accent.
        let (glyph, color) = match (row.status, row.unseen) {
            (Status::Working, _) => ("●", t.green),
            (Status::Idle, true) => ("◆", t.accent),
            (Status::Idle, false) => ("○", t.muted),
            (Status::Gone, _) => ("✗", t.red),
        };
        let bar = if selected {
            Span::styled("▌", Style::default().fg(t.accent))
        } else {
            Span::raw(" ")
        };

        let stat = if row.added > 0 || row.removed > 0 {
            format!("+{} −{}", row.added, row.removed)
        } else {
            String::new()
        };
        let name_width = width.saturating_sub(4 + stat.chars().count());
        let name = fit(&row.ws.branch, name_width);
        let name_style = if selected {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        // Both lines are padded to exactly the inner width, so a selected row's
        // ground reaches the full column.
        let ground = if selected {
            Style::default().bg(t.selection)
        } else {
            Style::default()
        };

        lines.push(
            Line::from(vec![
                bar.clone(),
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(format!("{name:<name_width$}"), name_style),
                Span::styled(format!(" {stat}"), dim(&t)),
            ])
            .style(ground),
        );

        let shown = match row.status {
            Status::Idle => elapsed(row.since),
            _ => elapsed(row.age),
        };
        let meta_width = width.saturating_sub(4 + shown.chars().count());
        let meta = fit(&format!("{} · {}", row.ws.repo, row.ws.agent), meta_width);
        lines.push(
            Line::from(vec![
                bar,
                Span::raw("  "),
                Span::styled(format!("{meta:<meta_width$}"), dim(&t)),
                Span::styled(format!("{shown} "), dim(&t)),
            ])
            .style(ground),
        );
        lines.push(Line::from(""));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_main(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let insert = matches!(app.mode, Mode::Insert);
    let overlaid = app.overlay.is_some();
    let branch = app
        .selected_row()
        .map(|r| r.ws.branch.clone())
        .unwrap_or_default();
    let title = if overlaid {
        format!(" {} · {} ", app.overlay_title, branch)
    } else {
        match app.selected_row() {
            Some(row) => format!(" {} · {} · {} ", row.ws.branch, row.ws.repo, row.ws.agent),
            None => " agent ".to_string(),
        }
    };
    let insert = insert || overlaid;
    let border_style = if insert {
        Style::default().fg(t.accent)
    } else {
        dim(&t)
    };
    let title_style = if insert {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else {
        dim(&t)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, title_style));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.main_area = inner;

    if let Some(overlay) = app.overlay.as_ref()
        && let Some(parser) = overlay.parser()
    {
        frame.render_widget(PseudoTerminal::new(parser.screen()), inner);
        return;
    }

    let gone = matches!(app.selected_row().map(|r| r.status), Some(Status::Gone));
    if gone && app.pty.is_none() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  this agent's session is gone",
                Style::default().fg(t.red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  R restarts the agent here · d removes the workspace",
                dim(&t),
            )),
        ]);
        frame.render_widget(msg, inner);
        return;
    }

    if let Some(pty) = app.pty.as_ref()
        && let Some(parser) = pty.parser()
    {
        frame.render_widget(PseudoTerminal::new(parser.screen()), inner);
        return;
    }

    let hint = if app.rows.is_empty() {
        "  press n to start a workspace"
    } else {
        "  attaching…"
    };
    frame.render_widget(Paragraph::new(Span::styled(hint, dim(&t))), inner);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    if let Some(msg) = &app.message {
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {msg}"), Style::default().fg(t.red))),
            area,
        );
        return;
    }

    let line = match app.mode {
        Mode::Insert => Line::from(vec![
            Span::styled(
                " ─ INSERT ─",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  keys go to the agent    alt+n new   alt+j/k switch   ctrl+q leave",
                dim(&t),
            ),
        ]),
        Mode::New(_) => Line::from(Span::styled(" ⏎ next   esc back", dim(&t))),
        Mode::Confirm => Line::from(Span::styled(" y remove   n keep", dim(&t))),
        Mode::Land => Line::from(Span::styled(" y merge and retire   n cancel", dim(&t))),
        Mode::Ports => match app.port_pending {
            Some(port) => {
                let kind = app
                    .ports
                    .iter()
                    .find(|p| p.port == port)
                    .map(|p| p.kind.clone())
                    .unwrap_or_else(|| "listener".into());
                Line::from(vec![
                    Span::styled(
                        format!(" stop {kind} on {port}?"),
                        Style::default().fg(t.red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   y confirm   n cancel", dim(&t)),
                ])
            }
            None => Line::from(Span::styled(
                " j/k move   x stop   r refresh   esc close",
                dim(&t),
            )),
        },
        Mode::Normal => {
            let gone = matches!(app.selected_row().map(|r| r.status), Some(Status::Gone));
            let keys = if gone {
                " j/k move   R restart   d delete   n new   q quit"
            } else {
                " j/k ⏎ insert  n new  g git  s sh  e ed  p ports  L land  d del  q quit"
            };
            Line::from(Span::styled(keys, dim(&t)))
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_new(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let Mode::New(form) = &app.mode else {
        return;
    };

    let (title, mut lines): (&str, Vec<Line>) = match form.step {
        Step::Repo => (" repository ", {
            if form.repos.is_empty() {
                vec![
                    Line::from(Span::styled(
                        "  no git repositories found",
                        Style::default().fg(t.red),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  set MAESTRO_REPO_ROOT to where you keep them",
                        dim(&t),
                    )),
                ]
            } else {
                // The basename identifies the repo; the parent only disambiguates,
                // so it is dimmed and clipped from the left.
                const ROW: usize = 48;
                form.repos
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let parent = p.parent().map(short_path).unwrap_or_default();
                        // Reserve the gap before clipping, so the name and the
                        // parent never run together on a deep path.
                        const GAP: usize = 3;
                        let width = ROW.saturating_sub(name.chars().count() + 3 + GAP);
                        let parent = tail(&parent, width);
                        let pad = ROW
                            .saturating_sub(name.chars().count() + parent.chars().count() + 3)
                            .max(GAP);
                        let selected = i == form.repo_sel;
                        Line::from(vec![
                            if selected {
                                Span::styled("▌ ", Style::default().fg(t.accent))
                            } else {
                                Span::raw("  ")
                            },
                            Span::styled(
                                name,
                                if selected {
                                    Style::default().add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default()
                                },
                            ),
                            Span::raw(" ".repeat(pad)),
                            Span::styled(parent, dim(&t)),
                        ])
                    })
                    .collect()
            }
        }),
        Step::Branch => (" branch ", {
            let name = if form.branch_is_suggestion {
                Span::styled(form.branch.clone(), dim(&t))
            } else {
                Span::raw(form.branch.clone())
            };
            let hint = if form.branch_is_suggestion {
                "  ⏎ accept · tab for another · type to replace"
            } else {
                "  what this agent works on"
            };
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  ❯ ", Style::default().fg(t.accent)),
                    name,
                    Span::styled("▏", Style::default().fg(t.accent)),
                ]),
                Line::from(""),
                Line::from(Span::styled(hint, dim(&t))),
            ]
        }),
        Step::Task => (" task ", {
            let width = 44usize;
            let shown = if form.task.chars().count() > width {
                let chars: Vec<char> = form.task.chars().collect();
                chars[chars.len() - width..].iter().collect::<String>()
            } else {
                form.task.clone()
            };
            vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  ❯ ", Style::default().fg(t.accent)),
                    Span::raw(shown),
                    Span::styled("▏", Style::default().fg(t.accent)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "  ⏎ start · empty to open the agent idle",
                    dim(&t),
                )),
            ]
        }),
        Step::Agent => (" agent ", {
            form.agents
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    if i == form.agent_sel {
                        Line::from(vec![
                            Span::styled("▌ ", Style::default().fg(t.accent)),
                            Span::styled(a.clone(), Style::default().add_modifier(Modifier::BOLD)),
                        ])
                    } else {
                        Line::from(format!("  {a}"))
                    }
                })
                .collect()
        }),
    };

    lines.insert(0, Line::from(""));
    let height = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(5);
    let rect = centered(52, height, area);

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            title,
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let Some(row) = app.selected_row() else {
        return;
    };
    let rect = centered(54, 7, area);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.red))
        .title(Span::styled(
            " remove workspace ",
            Style::default().fg(t.red).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", row.ws.id),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  deletes the worktree and the branch",
                dim(&t),
            )),
        ])
        .alignment(Alignment::Left),
        inner,
    );
}

fn render_land(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let Some((branch, base)) = app.land_target() else {
        return;
    };
    let rect = centered(58, 8, area);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " land the work ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("  merge "),
                Span::styled(branch, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" into "),
                Span::styled(base, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  then remove the worktree and the branch",
                dim(&t),
            )),
            Line::from(Span::styled(
                "  refuses unless both are clean and on the base",
                dim(&t),
            )),
        ]),
        inner,
    );
}

fn render_ports(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme;
    let height = (app.ports.len() as u16 + 6)
        .min(area.height.saturating_sub(2))
        .max(8);
    let rect = centered(78, height, area);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(Span::styled(
            " listening ports ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![Line::from("")];

    if app.ports.is_empty() {
        lines.push(Line::from(Span::styled(
            "  nothing of yours is listening",
            dim(&t),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let visible = inner.height.saturating_sub(4) as usize;
    let offset = app.port_sel.saturating_sub(visible.saturating_sub(1));

    for (i, entry) in app.ports.iter().enumerate().skip(offset).take(visible) {
        let selected = i == app.port_sel;
        let bar = if selected {
            Span::styled("▌", Style::default().fg(t.accent))
        } else {
            Span::raw(" ")
        };
        // The CLI decides what counts as a dev server; dev rows lead the eye.
        let (mark, mark_style) = if entry.dev {
            ("●", Style::default().fg(t.accent))
        } else {
            ("·", dim(&t))
        };
        let project = entry.project.clone().unwrap_or_else(|| "-".into());
        let held = if entry.ports_held > 1 {
            format!("×{}", entry.ports_held)
        } else {
            String::new()
        };
        let text = format!(
            "{:<6} {} {} {:>5}  {}",
            entry.port,
            fit(&entry.kind, 11),
            fit(&project, 20),
            entry.uptime,
            held
        );
        let style = if selected {
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        lines.push(Line::from(vec![
            bar,
            Span::styled(format!(" {mark} "), mark_style),
            Span::styled(text, style),
        ]));
    }

    // The command line of the selected row, which is what actually identifies it.
    if let Some(entry) = app.ports.get(app.port_sel) {
        lines.push(Line::from(""));
        let room = inner.width.saturating_sub(4) as usize;
        let detail = format!("pid {} · {}", entry.pid, entry.command);
        lines.push(Line::from(Span::styled(
            format!("  {}", fit(&detail, room)),
            dim(&t),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
