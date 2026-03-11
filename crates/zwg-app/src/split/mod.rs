//! Split pane tree — manages horizontal/vertical terminal splits

use gpui::*;
use uuid::Uuid;

use crate::terminal::{TerminalPane, TerminalSettings};

/// Direction of a split
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitDirection {
    Horizontal, // side by side (left | right)
    Vertical,   // stacked (top / bottom)
}

/// Node in the split tree
enum SplitNode {
    Leaf {
        id: Uuid,
        terminal: Entity<TerminalPane>,
    },
    Branch {
        direction: SplitDirection,
        ratio: f32, // 0.0..1.0, fraction for first child
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

/// Split pane container — manages a tree of terminal panes
pub struct SplitContainer {
    root: SplitNode,
    focused_id: Uuid,
    shell: String,
    terminal_settings: TerminalSettings,
    resize_drag: Option<ResizeDragState>,
}

struct ResizeDragState {
    branch_path: Vec<bool>,
    direction: SplitDirection,
    last_position: Point<Pixels>,
}

impl SplitContainer {
    pub fn new(shell: &str, terminal_settings: TerminalSettings, cx: &mut Context<Self>) -> Self {
        let id = Uuid::new_v4();
        let terminal = cx.new(|cx| TerminalPane::new(shell, terminal_settings.clone(), cx));
        Self {
            root: SplitNode::Leaf { id, terminal },
            focused_id: id,
            shell: shell.to_string(),
            terminal_settings,
            resize_drag: None,
        }
    }

    /// Split the focused pane in the given direction
    pub fn split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        let target_id = self.focused_id;
        let shell = self.shell.clone();
        let new_id = Uuid::new_v4();
        let terminal_settings = self.terminal_settings.clone();
        let new_terminal = cx.new(|cx| TerminalPane::new(&shell, terminal_settings, cx));

        self.root = Self::split_node(
            std::mem::replace(
                &mut self.root,
                SplitNode::Leaf {
                    id: Uuid::nil(),
                    terminal: new_terminal.clone(),
                },
            ),
            target_id,
            direction,
            new_id,
            new_terminal,
        );

        self.focused_id = new_id;
        cx.notify();
    }

    fn split_node(
        node: SplitNode,
        target_id: Uuid,
        direction: SplitDirection,
        new_id: Uuid,
        new_terminal: Entity<TerminalPane>,
    ) -> SplitNode {
        match node {
            SplitNode::Leaf { id, terminal } if id == target_id => SplitNode::Branch {
                direction,
                ratio: 0.5,
                first: Box::new(SplitNode::Leaf { id, terminal }),
                second: Box::new(SplitNode::Leaf {
                    id: new_id,
                    terminal: new_terminal,
                }),
            },
            SplitNode::Branch {
                direction: d,
                ratio,
                first,
                second,
            } => SplitNode::Branch {
                direction: d,
                ratio,
                first: Box::new(Self::split_node(
                    *first,
                    target_id,
                    direction,
                    new_id,
                    new_terminal.clone(),
                )),
                second: Box::new(Self::split_node(
                    *second,
                    target_id,
                    direction,
                    new_id,
                    new_terminal,
                )),
            },
            other => other,
        }
    }

    /// Get the first terminal Entity (cheap clone, no PTY spawn)
    fn first_terminal(&self) -> Entity<TerminalPane> {
        Self::get_first_terminal(&self.root)
    }

    fn get_first_terminal(node: &SplitNode) -> Entity<TerminalPane> {
        match node {
            SplitNode::Leaf { terminal, .. } => terminal.clone(),
            SplitNode::Branch { first, .. } => Self::get_first_terminal(first),
        }
    }

    /// Close the focused pane. Returns false if it's the last pane.
    pub fn close_focused(&mut self, cx: &mut Context<Self>) -> bool {
        let target_id = self.focused_id;

        if let SplitNode::Leaf { id, .. } = &self.root {
            if *id == target_id {
                return false; // last pane, can't close
            }
        }

        // Use existing terminal clone as dummy (no new PTY spawned)
        let dummy_terminal = self.first_terminal();
        let (new_root, sibling_id) = Self::remove_node(
            std::mem::replace(
                &mut self.root,
                SplitNode::Leaf {
                    id: Uuid::nil(),
                    terminal: dummy_terminal,
                },
            ),
            target_id,
        );

        if let Some(root) = new_root {
            self.root = root;
            if let Some(sid) = sibling_id {
                self.focused_id = sid;
            }
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Remove a node from the tree, returning the new tree and the sibling's first leaf ID
    fn remove_node(node: SplitNode, target_id: Uuid) -> (Option<SplitNode>, Option<Uuid>) {
        match node {
            SplitNode::Branch {
                direction,
                ratio,
                first,
                second,
            } => {
                // Check if first child is the target
                if let SplitNode::Leaf { id, .. } = first.as_ref() {
                    if *id == target_id {
                        let sid = Self::first_leaf_id(&second);
                        return (Some(*second), Some(sid));
                    }
                }
                // Check if second child is the target
                if let SplitNode::Leaf { id, .. } = second.as_ref() {
                    if *id == target_id {
                        let sid = Self::first_leaf_id(&first);
                        return (Some(*first), Some(sid));
                    }
                }
                // Recurse into children — preserve original direction and ratio
                let (new_first, sid1) = Self::remove_node(*first, target_id);
                if let Some(nf) = new_first {
                    if sid1.is_some() {
                        return (
                            Some(SplitNode::Branch {
                                direction,
                                ratio,
                                first: Box::new(nf),
                                second,
                            }),
                            sid1,
                        );
                    }
                    let (new_second, sid2) = Self::remove_node(*second, target_id);
                    if let Some(ns) = new_second {
                        return (
                            Some(SplitNode::Branch {
                                direction,
                                ratio,
                                first: Box::new(nf),
                                second: Box::new(ns),
                            }),
                            sid2,
                        );
                    }
                }
                (None, None)
            }
            other => (Some(other), None),
        }
    }

    fn first_leaf_id(node: &SplitNode) -> Uuid {
        match node {
            SplitNode::Leaf { id, .. } => *id,
            SplitNode::Branch { first, .. } => Self::first_leaf_id(first),
        }
    }

    /// Get all terminal entities in order
    pub fn all_terminals(&self) -> Vec<(Uuid, Entity<TerminalPane>)> {
        let mut result = Vec::new();
        Self::collect_terminals(&self.root, &mut result);
        result
    }

    pub fn focused_terminal(&self) -> Option<Entity<TerminalPane>> {
        Self::find_terminal(&self.root, self.focused_id)
    }

    fn collect_terminals(node: &SplitNode, out: &mut Vec<(Uuid, Entity<TerminalPane>)>) {
        match node {
            SplitNode::Leaf { id, terminal } => out.push((*id, terminal.clone())),
            SplitNode::Branch { first, second, .. } => {
                Self::collect_terminals(first, out);
                Self::collect_terminals(second, out);
            }
        }
    }

    fn find_terminal(node: &SplitNode, target_id: Uuid) -> Option<Entity<TerminalPane>> {
        match node {
            SplitNode::Leaf { id, terminal } if *id == target_id => Some(terminal.clone()),
            SplitNode::Leaf { .. } => None,
            SplitNode::Branch { first, second, .. } => Self::find_terminal(first, target_id)
                .or_else(|| Self::find_terminal(second, target_id)),
        }
    }

    /// Focus the next pane in the given direction
    pub fn focus_direction(&mut self, dir: FocusDir, cx: &mut Context<Self>) {
        let terminals = self.all_terminals();
        if terminals.len() <= 1 {
            return;
        }
        let current_idx = terminals
            .iter()
            .position(|(id, _)| *id == self.focused_id)
            .unwrap_or(0);

        let new_idx = match dir {
            FocusDir::Next => (current_idx + 1) % terminals.len(),
            FocusDir::Prev => {
                if current_idx == 0 {
                    terminals.len() - 1
                } else {
                    current_idx - 1
                }
            }
        };

        self.focused_id = terminals[new_idx].0;
        cx.notify();
    }

    pub fn focused_id(&self) -> Uuid {
        self.focused_id
    }

    pub fn update_terminal_settings(&mut self, terminal_settings: TerminalSettings) {
        self.terminal_settings = terminal_settings;
    }

    fn begin_resize_drag(
        &mut self,
        branch_path: Vec<bool>,
        direction: SplitDirection,
        start: Point<Pixels>,
    ) {
        self.resize_drag = Some(ResizeDragState {
            branch_path,
            direction,
            last_position: start,
        });
    }

    fn end_resize_drag(&mut self) {
        self.resize_drag = None;
    }

    fn update_resize_drag(
        &mut self,
        position: Point<Pixels>,
        viewport_size: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        const MIN_RATIO: f32 = 0.15;
        const MAX_RATIO: f32 = 0.85;

        let Some(drag) = self.resize_drag.as_mut() else {
            return;
        };

        let one = px(1.0);
        let delta_px = match drag.direction {
            SplitDirection::Horizontal => (position.x - drag.last_position.x) / one,
            SplitDirection::Vertical => (position.y - drag.last_position.y) / one,
        };
        if delta_px.abs() < 0.01 {
            return;
        }
        drag.last_position = position;

        let axis_span = match drag.direction {
            SplitDirection::Horizontal => (viewport_size.width / one).max(1.0),
            SplitDirection::Vertical => (viewport_size.height / one).max(1.0),
        };
        let delta_ratio = delta_px / axis_span;

        if Self::adjust_branch_ratio_by_path(
            &mut self.root,
            &drag.branch_path,
            delta_ratio,
            MIN_RATIO,
            MAX_RATIO,
        ) {
            cx.notify();
        }
    }

    fn adjust_branch_ratio_by_path(
        node: &mut SplitNode,
        path: &[bool],
        delta_ratio: f32,
        min_ratio: f32,
        max_ratio: f32,
    ) -> bool {
        if path.is_empty() {
            if let SplitNode::Branch { ratio, .. } = node {
                let next = (*ratio + delta_ratio).clamp(min_ratio, max_ratio);
                if (next - *ratio).abs() > f32::EPSILON {
                    *ratio = next;
                    return true;
                }
            }
            return false;
        }

        match node {
            SplitNode::Branch { first, second, .. } => {
                if path[0] {
                    Self::adjust_branch_ratio_by_path(
                        second,
                        &path[1..],
                        delta_ratio,
                        min_ratio,
                        max_ratio,
                    )
                } else {
                    Self::adjust_branch_ratio_by_path(
                        first,
                        &path[1..],
                        delta_ratio,
                        min_ratio,
                        max_ratio,
                    )
                }
            }
            SplitNode::Leaf { .. } => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FocusDir {
    Next,
    Prev,
}

impl Render for SplitContainer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused_id = self.focused_id;
        Self::render_node(&self.root, focused_id, cx.entity().clone(), &[])
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if ev.dragging() {
                    this.update_resize_drag(ev.position, window.viewport_size(), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, _cx| {
                    this.end_resize_drag();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, _cx| {
                    this.end_resize_drag();
                }),
            )
    }
}

impl SplitContainer {
    fn render_node(
        node: &SplitNode,
        focused_id: Uuid,
        pane_entity: Entity<SplitContainer>,
        branch_path: &[bool],
    ) -> Div {
        match node {
            SplitNode::Leaf { id, terminal } => {
                let is_focused = *id == focused_id;
                let mut el = div().size_full().child(terminal.clone());
                if is_focused {
                    el = el.border_2().border_color(rgb(0x0A84FF));
                } else {
                    el = el.border_1().border_color(rgb(0x3C3C3E));
                }
                el
            }
            SplitNode::Branch {
                direction,
                ratio,
                first,
                second,
            } => {
                let mut first_path = branch_path.to_vec();
                first_path.push(false);
                let first_el =
                    Self::render_node(first, focused_id, pane_entity.clone(), &first_path);
                let mut second_path = branch_path.to_vec();
                second_path.push(true);
                let second_el =
                    Self::render_node(second, focused_id, pane_entity.clone(), &second_path);
                let r = *ratio;

                match direction {
                    SplitDirection::Horizontal => div()
                        .size_full()
                        .flex()
                        .flex_row()
                        .child(
                            first_el
                                .flex_grow()
                                .flex_basis(relative(r))
                                .min_w(px(140.0)),
                        )
                        .child({
                            let pane_resize = pane_entity.clone();
                            let path_for_start = branch_path.to_vec();
                            div()
                                .w(px(4.0))
                                .h_full()
                                .cursor_col_resize()
                                .on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
                                    pane_resize.update(cx, |pane, _cx| {
                                        pane.begin_resize_drag(
                                            path_for_start.clone(),
                                            SplitDirection::Horizontal,
                                            ev.position,
                                        );
                                    });
                                })
                                .child(div().mx(px(1.5)).w(px(1.0)).h_full().bg(rgb(0x3C3C3E)))
                        })
                        .child(
                            second_el
                                .flex_grow()
                                .flex_basis(relative(1.0 - r))
                                .min_w(px(140.0)),
                        ),
                    SplitDirection::Vertical => div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(first_el.flex_grow().flex_basis(relative(r)).min_h(px(96.0)))
                        .child({
                            let pane_resize = pane_entity.clone();
                            let path_for_start = branch_path.to_vec();
                            div()
                                .h(px(4.0))
                                .w_full()
                                .cursor_row_resize()
                                .on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
                                    pane_resize.update(cx, |pane, _cx| {
                                        pane.begin_resize_drag(
                                            path_for_start.clone(),
                                            SplitDirection::Vertical,
                                            ev.position,
                                        );
                                    });
                                })
                                .child(div().my(px(1.5)).h(px(1.0)).w_full().bg(rgb(0x3C3C3E)))
                        })
                        .child(
                            second_el
                                .flex_grow()
                                .flex_basis(relative(1.0 - r))
                                .min_h(px(96.0)),
                        ),
                }
            }
        }
    }
}

impl EventEmitter<()> for SplitContainer {}
