//! Màn hình live: cây graph bên trái, log của node đang chọn bên phải.

use agentgraph_core::view::View;
use crossterm::event::{self as cev, Event as CEvent, KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::time::Duration;

pub struct Ui {
    pub view: View,
    pub sel: usize,
    pub scroll: u16,
}

impl Ui {
    pub fn new() -> Self {
        Self {
            view: View::default(),
            sel: 0,
            scroll: 0,
        }
    }

    fn sel_lines(&self) -> usize {
        self.view
            .order
            .get(self.sel)
            .and_then(|i| self.view.nodes.get(i))
            .map(|n| n.lines.len())
            .unwrap_or(0)
    }

    /// `scroll` đếm ngược từ dòng cuối, nên PageUp là *tăng*: lùi về quá khứ.
    /// Chặn trần ở số dòng đang có, nếu không cuộn quá tay ra vùng trắng và
    /// người dùng tưởng log trống.
    fn handle_page(&mut self, up: bool) {
        self.scroll = if up {
            let max = self.sel_lines().saturating_sub(1) as u16;
            self.scroll.saturating_add(10).min(max)
        } else {
            self.scroll.saturating_sub(10)
        };
    }

    /// Trả về false khi người dùng muốn thoát.
    pub fn handle_input(&mut self, timeout: Duration) -> anyhow::Result<bool> {
        if !cev::poll(timeout)? {
            return Ok(true);
        }
        if let CEvent::Key(k) = cev::read()? {
            if k.kind != cev::KeyEventKind::Press {
                return Ok(true);
            }
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(false);
                }
                KeyCode::Down | KeyCode::Char('j') if self.sel + 1 < self.view.order.len() => {
                    self.sel += 1;
                    self.scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.sel = self.sel.saturating_sub(1);
                    self.scroll = 0;
                }
                KeyCode::PageUp => self.handle_page(true),
                KeyCode::PageDown => self.handle_page(false),
                _ => {}
            }
        }
        Ok(true)
    }

    pub fn draw(&self, f: &mut Frame) {
        let root = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

        let (run, done, fail, wait) = self.view.counts();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " agentgraph ",
                    Style::new().bold().bg(Color::Blue).fg(Color::White),
                ),
                Span::raw(" "),
                Span::styled(
                    self.view.goal.chars().take(80).collect::<String>(),
                    Style::new().fg(Color::Gray),
                ),
            ])),
            root[0],
        );

        let cols = Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(root[1]);

        // ── cây graph ──────────────────────────────────────────────
        let items: Vec<ListItem> = self
            .view
            .order
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let n = self.view.nodes.get(id)?;
                let (mark, color) = match n.state.as_str() {
                    "running" => ("◐", Color::Yellow),
                    "done" => ("●", Color::Green),
                    "failed" => ("✕", Color::Red),
                    "skipped" => ("⊘", Color::DarkGray),
                    "ready" => ("○", Color::Cyan),
                    _ => ("○", Color::DarkGray),
                };
                let sel = i == self.sel;
                let indent = if n.deps.is_empty() { "" } else { "  " };
                // Con trỏ phải là ký tự, không chỉ là màu: bold trắng gần như
                // vô hình trên nhiều theme terminal.
                let mut spans = vec![
                    Span::styled(
                        if sel { "▸" } else { " " },
                        Style::new().fg(Color::Cyan).bold(),
                    ),
                    Span::raw(indent),
                    Span::styled(format!("{mark} "), Style::new().fg(color)),
                    Span::styled(
                        n.id.clone(),
                        if sel {
                            Style::new().bold().fg(Color::White)
                        } else {
                            Style::new()
                        },
                    ),
                    Span::styled(format!("  {}", n.agent), Style::new().fg(Color::DarkGray)),
                ];
                if n.by_agent {
                    spans.push(Span::styled(" ⚙", Style::new().fg(Color::Magenta)));
                }
                if n.cost > 0.0 {
                    spans.push(Span::styled(
                        format!("  ${:.2}", n.cost),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
                Some(ListItem::new(Line::from(spans)))
            })
            .collect();

        // Vẽ có state để danh sách tự cuộn theo lựa chọn: graph dài hơn khung
        // thì node đang chọn — và con trỏ ▸ — nằm ngoài màn hình, điều hướng
        // thành mù.
        let mut list_state = ListState::default().with_selected(Some(self.sel));
        f.render_stateful_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" graph · {} node ", self.view.order.len())),
            ),
            cols[0],
            &mut list_state,
        );

        // ── log node đang chọn ─────────────────────────────────────
        let sel_id = self.view.order.get(self.sel);
        let (title, body) = match sel_id.and_then(|i| self.view.nodes.get(i)) {
            Some(n) => {
                let h = cols[1].height.saturating_sub(2) as usize;
                let start = n.lines.len().saturating_sub(h + self.scroll as usize);
                let end = n.lines.len().saturating_sub(self.scroll as usize);
                let head = if n.title.is_empty() || n.title == n.id {
                    format!(" {} · {} · {} ", n.id, n.agent, n.state)
                } else {
                    format!(" {} — {} · {} · {} ", n.id, n.title, n.agent, n.state)
                };
                (head, n.lines[start.min(end)..end].join("\n"))
            }
            None => (" log ".into(), "chưa có node nào".into()),
        };
        f.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(title)),
            cols[1],
        );

        // ── thanh trạng thái ───────────────────────────────────────
        let mut status = vec![
            Span::styled(
                format!(" ${:.2} ", self.view.total_cost),
                Style::new().fg(Color::Green).bold(),
            ),
            Span::raw(format!(
                "· {run} chạy  {done} xong  {fail} hỏng  {wait} chờ "
            )),
        ];
        if self.view.mutations_rejected > 0 {
            status.push(Span::styled(
                format!("· {} mutation bị từ chối ", self.view.mutations_rejected),
                Style::new().fg(Color::Magenta),
            ));
        }
        if self.view.finished {
            status.push(Span::styled(
                "· XONG ",
                Style::new().fg(Color::Green).bold(),
            ));
        }
        status.push(Span::styled(
            "· [↑↓] node  [q] thoát",
            Style::new().fg(Color::DarkGray),
        ));
        f.render_widget(Paragraph::new(Line::from(status)), root[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentgraph_core::event::{Event, EventKind, Origin};
    use agentgraph_core::ids::NodeId;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn ev(seq: u64, node: &str, kind: EventKind) -> Event {
        Event {
            seq,
            at: chrono::Utc::now(),
            node: Some(NodeId::new(node).unwrap()),
            kind,
        }
    }

    /// `n` node, node đầu có `lines` dòng log.
    fn ui_voi(n: usize, lines: usize) -> Ui {
        let mut ui = Ui::new();
        for i in 0..n {
            let id = format!("node-{i:02}");
            ui.view.apply(&ev(
                i as u64,
                &id,
                EventKind::NodeAdded {
                    title: id.clone(),
                    agent: "fake".into(),
                    deps: vec![],
                    by: Origin::Plan,
                },
            ));
        }
        for i in 0..lines {
            ui.view.apply(&ev(
                1000 + i as u64,
                "node-00",
                EventKind::AgentText {
                    text: format!("dòng {i}"),
                },
            ));
        }
        ui
    }

    fn ve(ui: &Ui, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui.draw(f)).unwrap();
        t.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn node_dang_chon_luon_nhin_thay_du_graph_dai_hon_khung() {
        // Danh sách không cuộn theo lựa chọn thì con trỏ biến mất khỏi màn
        // hình và điều hướng thành mù.
        let mut ui = ui_voi(40, 0);
        ui.sel = 39;
        let s = ve(&ui, 80, 20);
        assert!(s.contains("▸"), "con trỏ phải còn thấy được");
        assert!(s.contains("node-39"), "node đang chọn phải nằm trong khung");
    }

    #[test]
    fn page_up_lui_ve_qua_khu_page_down_tra_lai_cuoi() {
        let mut ui = ui_voi(1, 100);
        assert_eq!(ui.scroll, 0);
        ui.scroll = 0;
        ui.handle_page(true);
        assert_eq!(ui.scroll, 10, "PageUp phải lùi về dòng cũ");
        ui.handle_page(false);
        assert_eq!(ui.scroll, 0, "PageDown phải quay lại cuối log");
    }

    #[test]
    fn khong_cuon_qua_so_dong_dang_co() {
        // Cuộn quá tay từng cho ra khung log trắng trơn không lý do.
        let mut ui = ui_voi(1, 12);
        for _ in 0..20 {
            ui.handle_page(true);
        }
        assert_eq!(ui.scroll, 11);
        let s = ve(&ui, 80, 20);
        assert!(s.contains("dòng 0"), "phải còn thấy log chứ không trắng");
    }
}
