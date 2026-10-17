//! Headless editor fixtures and regression tests for application commands.

use crate::workspace::{Workspace, commands::Command};
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{test::TestWindowExt, *};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) const SAMPLE_FILES: &[(&str, &str)] = &[
    (
        "main.typ",
        include_str!("../tests/fixtures/manuscript/main.typ"),
    ),
    (
        "chapters/introduction.typ",
        include_str!("../tests/fixtures/manuscript/chapters/introduction.typ"),
    ),
    (
        "chapters/model.typ",
        include_str!("../tests/fixtures/manuscript/chapters/model.typ"),
    ),
    (
        "notes.typ",
        include_str!("../tests/fixtures/manuscript/notes.typ"),
    ),
];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Step {
    #[serde(flatten)]
    pub event: Event,
    /// An intentional invalid command must be rejected, without a panic.
    #[serde(default)]
    pub expect_error: bool,
    /// Recursive subset of the state required after this step has settled.
    #[serde(default)]
    pub expect: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum Event {
    Workspace {
        value: Command,
    },
    Action {
        name: String,
        #[serde(default)]
        params: Option<Value>,
    },
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Click {
        id: Value,
        #[serde(default)]
        scope: Option<Value>,
    },
    Hover {
        id: Value,
    },
    Scroll {
        id: Value,
        x: f32,
        y: f32,
    },
    Drag {
        from: [f32; 2],
        to: [f32; 2],
    },
    Resize {
        width: f32,
        height: f32,
    },
    /// Changes the target of subsequent events; "preview" requires a popout.
    Window {
        target: String,
    },
    CloseWindow,
    Wait {
        until: Value,
        #[serde(default = "timeout")]
        timeout_ms: u64,
    },
    Assert {
        state: Value,
    },
    AdvanceTime {
        milliseconds: u64,
    },
    Catalog,
    Snapshot,
    Screenshot {
        path: String,
    },
}
fn timeout() -> u64 {
    20_000
}

// Production windows share a platform font system. Keep the same ownership
// here instead of repeatedly reopening the font database for replay sessions.
fn test_text_system() -> Arc<dyn PlatformTextSystem> {
    thread_local! {
        static TEXT: Arc<dyn PlatformTextSystem> = platform::current_platform(true).text_system();
    }
    TEXT.with(Arc::clone)
}

pub(crate) struct Runner {
    workspace: Entity<Workspace>,
    // Release our content handle before App clears its windows. Otherwise
    // editor timers can survive into the headless scheduler's shutdown.
    app: HeadlessAppContext,
    main: AnyWindowHandle,
    target: AnyWindowHandle,
    // Drop the app and signal worker shutdown before deleting the fixtures.
    _directory: tempfile::TempDir,
    #[cfg(test)]
    pub root: std::path::PathBuf,
}
impl Runner {
    pub fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("manuscript");
        fs::create_dir_all(root.join("chapters"))?;
        for (path, source) in SAMPLE_FILES {
            fs::write(root.join(path), source)?;
        }
        let settings_path = directory.path().join("settings.json");
        let mut app = HeadlessAppContext::with_platform(
            test_text_system(),
            Arc::new(crate::branding::Assets),
            platform::current_headless_renderer,
        );
        app.update(|cx| {
            init(cx);
            crate::configure_theme(cx);
            crate::bind_editor_keys(cx);
        });
        let (main, workspace) = app.update(|cx| {
            open_window(
                WindowOptions {
                    show: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1440.), px(940.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        Workspace::with_paths(
                            root.clone(),
                            Default::default(),
                            Some(settings_path),
                            None,
                            window,
                            cx,
                        )
                    })
                },
            )
        })?;
        let mut runner = Self {
            app,
            workspace,
            main,
            target: main,
            _directory: directory,
            #[cfg(test)]
            root,
        };
        runner.event(&Event::Workspace {
            value: Command::FocusEditor {},
        })?;
        runner.settle()?;
        Ok(runner)
    }
    fn settle(&mut self) -> Result<()> {
        for _ in 0..3 {
            self.app.run_until_parked();
            let windows = self.app.update(|cx| cx.windows());
            for window in windows {
                self.app
                    .update_window(window, |_, window, cx| window.render_frame(cx))?;
            }
        }
        self.app
            .update(|cx| self.workspace.read(cx).check_invariants(cx))
    }
    pub fn state(&mut self) -> Value {
        self.app.update(|cx| {
            let mut state = self.workspace.read(cx).snapshot(cx);
            state["window_count"] = json!(cx.windows().len());
            state["target_window"] = json!(if !cx.windows().contains(&self.target) {
                "closed"
            } else if self.target == self.main {
                "main"
            } else {
                "preview"
            });
            state
        })
    }
    /// Do not catch panics: crashes must retain their native stack and nonzero
    /// exit. The CLI flushes each input before dispatch for replay of failures.
    pub fn step(&mut self, step: &Step) -> Result<Value> {
        let before = self.state();
        let result = self.event(&step.event);
        self.settle().context("Application invariant failed")?;
        let after = self.state();
        let accepted = result.is_ok();
        let error = result.as_ref().err().map(|error| format!("{error:#}"));
        let expectation_error = if step.expect_error == accepted {
            Some(if accepted {
                "Expected a rejection, but command succeeded".to_owned()
            } else {
                format!(
                    "Unexpected command rejection: {}",
                    error.as_deref().unwrap_or_default()
                )
            })
        } else {
            step.expect
                .as_ref()
                .and_then(|expect| subset(&after, expect, "state").err().map(|e| e.to_string()))
        };
        // State snapshots include both deferred error channels and every buffer.
        // This lets callers distinguish a no-op,
        // an accepted async request and a completed operation.
        Ok(
            json!({"input": step, "accepted": accepted, "result": result.ok(), "error": error,
            "changed": observable(&before) != observable(&after), "state": after, "expectation_error": expectation_error}),
        )
    }
    fn event(&mut self, event: &Event) -> Result<Value> {
        match event {
            Event::Snapshot => return Ok(self.state()),
            Event::AdvanceTime { milliseconds } => {
                ensure!(
                    *milliseconds <= 60_000,
                    "Clock advance must not exceed 60000 ms"
                );
                self.app.advance_clock(Duration::from_millis(*milliseconds));
            }
            Event::Assert { state } => {
                subset(&self.state(), state, "state")?;
            }
            Event::Wait { until, timeout_ms } => {
                ensure!(
                    *timeout_ms <= 60_000,
                    "Wait timeout must not exceed 60000 ms"
                );
                let deadline = Instant::now() + Duration::from_millis(*timeout_ms);
                loop {
                    self.settle()?;
                    if subset(&self.state(), until, "state").is_ok() {
                        break;
                    }
                    ensure!(
                        Instant::now() < deadline,
                        "Timed out waiting for {until}; observed {}",
                        self.state()
                    );
                    self.app.advance_clock(Duration::from_millis(5));
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Event::Window { target } => {
                let next = match target.as_str() {
                    "main" => self.main,
                    "preview" => self
                        .app
                        .update(|cx| self.workspace.read(cx).preview_window())
                        .context("Preview is not detached")?,
                    _ => anyhow::bail!("Window target must be main or preview"),
                };
                ensure!(
                    self.app.update(|cx| cx.windows().contains(&next)),
                    "Target window is closed"
                );
                self.target = next;
            }
            Event::Screenshot { path } => {
                self.app.capture_screenshot(self.target)?.save(path)?;
            }
            _ => {
                let workspace = self.workspace.clone();
                return self.app.update_window(self.target, |_, window, cx| -> Result<Value> {
                    match event {
                        Event::Workspace { value } => return workspace.update(cx, |this, cx| this.dispatch_command(value, window, cx)),
                        Event::Action { name, params } => {
                            let action = cx.build_action(name, params.clone())?;
                            ensure!(window.is_action_available(action.as_ref(), cx), "Action {name} has no handler at the current focus");
                            window.dispatch_action(action, cx);
                        }
                        Event::Key { key } => {
                            Keystroke::parse(key).map_err(|e| anyhow::anyhow!("Invalid keystroke: {e}"))?;
                            window.press(key, cx);
                        }
                        Event::Text { text } => window.input(text, cx),
                        Event::Click { id, scope } => {
                            let id = element_id(id)?;
                            if let Some(scope) = scope {
                                let scope = element_id(scope)?;
                                visible(window, &scope)?;
                                let prefix = window.find(scope.clone()).path().to_vec();
                                unique(window, &id, &prefix)?;
                                let mut scoped = window.within(scope);
                                ensure!(scoped.try_find(id.clone()).is_some_and(|e| e.visible()), "Click target is absent or hidden");
                                scoped.click(id, cx);
                            } else {
                                visible(window, &id)?;
                                window.click(id, cx);
                            }
                        }
                        Event::Hover { id } => {
                            let id = element_id(id)?; visible(window, &id)?; window.hover(id, cx);
                        }
                        Event::Scroll { id, x, y } => {
                            ensure!(x.is_finite() && y.is_finite(), "Scroll must be finite");
                            let id = element_id(id)?; visible(window, &id)?;
                            window.scroll(id, ScrollDelta::Pixels(point(px(*x), px(*y))), cx);
                        }
                        Event::Drag { from, to } => {
                            let size = window.viewport_size();
                            for p in [from, to] { ensure!(p.iter().all(|n| n.is_finite()) && p[0] >= 0. && p[1] >= 0. && px(p[0]) < size.width && px(p[1]) < size.height, "Drag position is outside the window"); }
                            window.drag(point(px(from[0]), px(from[1])), point(px(to[0]), px(to[1])), cx);
                        }
                        Event::Resize { width, height } => {
                            ensure!(width.is_finite() && height.is_finite() && (320.0..=4096.0).contains(width) && (240.0..=4096.0).contains(height), "Window dimensions must be between 320×240 and 4096×4096");
                            window.resize(size(px(*width), px(*height)));
                            window.bounds_changed(cx);
                        }
                        Event::CloseWindow => window.remove_window(),
                        Event::Catalog => return Ok(json!({"workspace_schema": schemars::schema_for!(Command), "registered_actions": cx.all_action_names(),
                            "available_actions": window.available_actions(cx).iter().map(|a| a.name()).collect::<Vec<_>>(),
                            "elements": gpui_kit::base::test_support::registered_paths(window)})),
                        _ => unreachable!("Non-window events are handled above"),
                    }
                    Ok(Value::Null)
                })?;
            }
        }
        Ok(Value::Null)
    }
}
fn observable(state: &Value) -> Value {
    let mut state = state.clone();
    state.as_object_mut().unwrap().remove("command_sequence");
    state
}
fn element_id(value: &Value) -> Result<ElementId> {
    if let Some(id) = value.as_str() {
        return Ok(SharedString::from(id.to_owned()).into());
    }
    if let Some(id) = value.as_u64() {
        return Ok(ElementId::Integer(id));
    }
    if let Some(pair) = value.as_array().filter(|a| a.len() == 2)
        && let (Some(name), Some(index)) = (pair[0].as_str(), pair[1].as_u64())
    {
        return Ok(ElementId::NamedInteger(name.to_owned().into(), index));
    }
    anyhow::bail!("Element ID must be a string, nonnegative integer, or [name, index]")
}
fn unique(window: &Window, id: &ElementId, scope: &[ElementId]) -> Result<()> {
    let count = gpui_kit::base::test_support::snapshots(window)
        .iter()
        .filter(|e| {
            e.path().len() > scope.len()
                && e.path().starts_with(scope)
                && e.path().last() == Some(id)
        })
        .count();
    ensure!(
        count == 1,
        "Expected one target {id:?}, found {count}; use a unique scope"
    );
    Ok(())
}
fn visible(window: &Window, id: &ElementId) -> Result<()> {
    unique(window, id, &[])?;
    ensure!(
        window.try_find(id.clone()).is_some_and(|e| e.visible()),
        "Target is absent or hidden: {id:?}"
    );
    Ok(())
}
fn subset(actual: &Value, expected: &Value, path: &str) -> Result<()> {
    if let Some(object) = expected.as_object() {
        for (key, value) in object {
            subset(&actual[key], value, &format!("{path}.{key}"))?;
        }
    } else {
        ensure!(
            actual == expected,
            "{path}: expected {expected}, found {actual}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn autosave_handles_multiple_buffers_and_keeps_each_save_error() {
        let mut runner = Runner::new().unwrap();
        let replace = |text: &str| Command::Replace {
            start: 0,
            end: 0,
            text: text.into(),
        };
        for index in 0..2 {
            runner
                .event(&Event::Workspace {
                    value: Command::OpenDocument { index },
                })
                .unwrap();
            runner
                .event(&Event::Workspace {
                    value: replace("// Saved automatically\n"),
                })
                .unwrap();
        }
        runner.app.run_until_parked();
        runner.app.advance_clock(Duration::from_millis(800));
        runner.app.run_until_parked();
        let state = runner.state();
        for document in state["documents"].as_array().unwrap().iter().take(2) {
            assert_eq!(document["dirty"], false);
            assert!(
                fs::read_to_string(runner.root.join(document["path"].as_str().unwrap()))
                    .unwrap()
                    .starts_with("// Saved automatically")
            );
        }
        fs::write(runner.root.join("main.typ"), "// External edit\n").unwrap();
        for index in 0..2 {
            runner
                .event(&Event::Workspace {
                    value: Command::OpenDocument { index },
                })
                .unwrap();
            runner
                .event(&Event::Workspace {
                    value: replace("// Another edit\n"),
                })
                .unwrap();
        }
        runner.app.run_until_parked();
        runner.app.advance_clock(Duration::from_millis(800));
        runner.app.run_until_parked();
        let state = runner.state();
        assert_eq!(state["documents"][0]["dirty"], true);
        assert!(
            state["documents"][0]["save_error"]
                .as_str()
                .unwrap()
                .contains("changed on disk")
        );
        assert_eq!(state["documents"][1]["dirty"], false);
        assert!(state["documents"][1]["save_error"].is_null());
        assert!(
            state["errors"]["save"]
                .as_str()
                .unwrap()
                .contains("changed on disk")
        );
        assert_eq!(
            fs::read_to_string(runner.root.join("main.typ")).unwrap(),
            "// External edit\n"
        );
    }
    fn run(runner: &mut Runner, input: Value) -> Value {
        let step: Step = serde_json::from_value(input).unwrap();
        let result = runner.step(&step).unwrap();
        assert!(result["expectation_error"].is_null(), "{result:#}");
        result
    }
    #[::core::prelude::v1::test]
    fn command_replay_validates_unicode_settings_tabs_and_save_errors() {
        let mut runner = Runner::new().unwrap();
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"replace","start":0,"end":0,"text":"é 😀\n"}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"select","start":1,"end":2},"expect_error":true}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"zoom","percent":0},"expect_error":true,"expect":{"zoom":1.0}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"configure","settings":{"font_size":1,"vim_enabled":true}},"expect_error":true,"expect":{"settings":{"font_size":13,"vim_enabled":false}}}),
        );
        for index in 0..4 {
            run(
                &mut runner,
                json!({"command":"workspace","value":{"op":"close_document","index":index}}),
            );
        }
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"focus_editor"},"expect_error":true,"expect":{"tabs":[]}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"open_document","index":0}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"open_document","index":9999},"expect_error":true}),
        );
        run(
            &mut runner,
            json!({"command":"action","name":"folio::Save","expect":{"errors":{"save":null}}}),
        );
        assert!(
            fs::read_to_string(runner.root.join("main.typ"))
                .unwrap()
                .starts_with("é 😀\n")
        );
        fs::remove_file(runner.root.join("main.typ")).unwrap();
        fs::create_dir(runner.root.join("main.typ")).unwrap();
        run(
            &mut runner,
            json!({"command":"action","name":"folio::Save"}),
        );
        assert!(
            runner.state()["errors"]["save"]
                .as_str()
                .unwrap()
                .contains("Could not save")
        );
        let settings = runner.root.parent().unwrap().join("settings.json");
        fs::create_dir(&settings).unwrap();
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"configure","settings":{"font_size":14}},"expect_error":true,"expect":{"settings":{"font_size":14}}}),
        );
        assert!(runner.state()["errors"]["settings"].is_string());
        fs::remove_dir(&settings).unwrap();
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"configure","settings":{"font_size":13}},"expect":{"errors":{"settings":null}}}),
        );
        let result = run(&mut runner, json!({"command":"catalog"}));
        assert!(
            result["result"]["registered_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name == "folio::RunCommand")
        );
        run(
            &mut runner,
            json!({"command":"action","name":"folio::NoSuchAction","expect_error":true}),
        );
        run(
            &mut runner,
            json!({"command":"click","id":"does-not-exist","expect_error":true}),
        );
        // Exercise the same JSON-parameterized action registered in the UI tree.
        run(
            &mut runner,
            json!({"command":"action","name":"folio::RunCommand","params":{"command":{"op":"zoom","percent":200}},"expect":{"zoom":2.0}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"configure","settings":{"vim_enabled":true}}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"replace","start":0,"end":0,"text":"x"},"expect_error":true}),
        );
        run(&mut runner, json!({"command":"key","key":"i"}));
        run(&mut runner, json!({"command":"text","text":"😀"}));
        run(&mut runner, json!({"command":"key","key":"escape"}));
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"vim_command","prefix":"/","text":"["}}),
        );
        assert!(runner.state()["documents"][0]["vim_message"].is_string());
    }
    #[::core::prelude::v1::test]
    fn command_replay_compilation_window_lifecycle_and_menu_extent() {
        let mut runner = Runner::new().unwrap();
        run(
            &mut runner,
            json!({"command":"wait","until":{"lsp_ready":true,"compiling":false,"errors":{"compile":null}}}),
        );
        run(&mut runner, json!({"command":"click","id":"zoom-menu"}));
        runner
            .app
            .update_window(runner.main, |_, window, cx| {
                let menu = window.find("popup-menu").bounds();
                let last = window.within("popup-menu").find(14usize).bounds();
                assert!(
                    last.bottom() <= menu.bottom(),
                    "Last preset must be visible without scrolling"
                );
                let before = last.origin;
                window.within("popup-menu").scroll(
                    14usize,
                    ScrollDelta::Pixels(point(px(0.), px(-200.))),
                    cx,
                );
                window.render_frame(cx);
                assert_eq!(
                    window.within("popup-menu").find(14usize).bounds().origin,
                    before,
                    "Menu must fit its contents"
                );
            })
            .unwrap();
        run(&mut runner, json!({"command":"key","key":"escape"}));
        let length = runner.state()["documents"][0]["text"]
            .as_str()
            .unwrap()
            .len();
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"replace","start":0,"end":length,"text":"#missing_folio_test_value"}}),
        );
        // Compilation errors are observable while the previous vector pages stay available.
        run(
            &mut runner,
            json!({"command":"wait","until":{"compiling":false,"compile_failed":true}}),
        );
        assert!(runner.state()["errors"]["compile"].is_string());
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"replace","start":0,"end":25,"text":"= Recovered\nHello é 😀"}}),
        );
        run(
            &mut runner,
            json!({"command":"wait","until":{"compiling":false,"preview_pages":1,"errors":{"compile":null}}}),
        );
        run(
            &mut runner,
            json!({"command":"action","name":"folio::PreviewFitWidth","expect":{"fit":"Width"}}),
        );
        run(
            &mut runner,
            json!({"command":"resize","width":1200,"height":700}),
        );
        run(
            &mut runner,
            json!({"command":"action","name":"folio::PopOutPreview","expect":{"preview_detached":true,"window_count":2}}),
        );
        run(&mut runner, json!({"command":"window","target":"preview"}));
        run(
            &mut runner,
            json!({"command":"action","name":"folio::PreviewFitPage","expect":{"fit":"Page"}}),
        );
        run(
            &mut runner,
            json!({"command":"close_window","expect":{"preview_detached":false,"window_count":1}}),
        );
        run(&mut runner, json!({"command":"window","target":"main"}));
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"focus_editor"}}),
        );
        run(
            &mut runner,
            json!({"command":"action","name":"folio::OpenSettings","expect":{"settings_open":true}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"close_panels"},"expect":{"settings_open":false}}),
        );
        run(
            &mut runner,
            json!({"command":"workspace","value":{"op":"focus_editor"}}),
        );
        run(
            &mut runner,
            json!({"command":"action","name":"folio::PopOutPreview"}),
        );
        run(
            &mut runner,
            json!({"command":"close_window","expect":{"window_count":0}}),
        );
    }
    #[::core::prelude::v1::test]
    fn malformed_commands_are_rejected_before_dispatch() {
        for input in [
            r#"{"op":"zoom","percent":null}"#,
            r#"{"op":"zoom","percent":1e999}"#,
            r#"{"op":"select","start":-1,"end":0}"#,
            r#"{"op":"configure","settings":{"font_size":999}}"#,
            r#"{"op":"configure","settings":{"typo":true}}"#,
            r#"{"op":"snapshot","surprise":1}"#,
            r#"{"op":"unknown"}"#,
        ] {
            assert!(serde_json::from_str::<Command>(input).is_err(), "{input}");
        }
        let step: Step = serde_json::from_value(json!({"command":"click","id":"zoom-in"})).unwrap();
        let encoded = serde_json::to_string(&step).unwrap();
        let _: Step = serde_json::from_str(&encoded).unwrap();
    }
}
