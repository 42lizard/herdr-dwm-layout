use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

type Result<T> = std::result::Result<T, String>;

const MIN_MFACT: f64 = 0.10;
const MAX_MFACT: f64 = 0.90;
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

trait Api {
    fn request(&mut self, method: &str, params: Value) -> Result<Value>;
}

struct HerdrClient {
    socket_path: PathBuf,
}

impl HerdrClient {
    fn from_environment() -> Self {
        let socket_path = env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config/herdr/herdr.sock"));
        Self { socket_path }
    }
}

impl Api for HerdrClient {
    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("dwm-{}-{sequence}", process::id());
        let request = json!({"id": request_id, "method": method, "params": params});
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|error| format!("connect {}: {error}", self.socket_path.display()))?;
        serde_json::to_writer(&mut stream, &request)
            .map_err(|error| format!("encode {method}: {error}"))?;
        stream
            .write_all(b"\n")
            .map_err(|error| format!("write {method}: {error}"))?;

        for line in BufReader::new(stream).lines() {
            let line = line.map_err(|error| format!("read {method}: {error}"))?;
            let response: Value =
                serde_json::from_str(&line).map_err(|error| format!("decode {method}: {error}"))?;
            if response.get("id").and_then(Value::as_str) != Some(&request_id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(format!("{method}: {error}"));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| format!("{method}: response has no result"));
        }
        Err(format!("{method}: socket closed without a response"))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct TabState {
    enabled: bool,
    orientation: Orientation,
    mfact: f64,
    nmaster: usize,
    last_master_pane: Option<String>,
    last_stack_pane: Option<String>,
    pane_ids: Vec<String>,
}

impl Default for TabState {
    fn default() -> Self {
        Self {
            enabled: true,
            orientation: Orientation::Horizontal,
            mfact: 0.5,
            nmaster: 1,
            last_master_pane: None,
            last_stack_pane: None,
            pane_ids: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
struct State {
    version: u32,
    tabs: BTreeMap<String, TabState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            tabs: BTreeMap::new(),
        }
    }
}

struct LockedState {
    _lock: File,
    path: PathBuf,
    state: State,
}

impl LockedState {
    fn open() -> Result<Self> {
        let directory = plugin_state_dir();
        fs::create_dir_all(&directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
        let lock_path = directory.join("state.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| format!("open {}: {error}", lock_path.display()))?;
        let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(format!(
                "lock {}: {}",
                lock_path.display(),
                std::io::Error::last_os_error()
            ));
        }
        let path = directory.join("state.json");
        let state = match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        Ok(Self {
            _lock: lock,
            path,
            state,
        })
    }

    fn save(&self) -> Result<()> {
        let temporary = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(&self.state)
            .map_err(|error| format!("encode state: {error}"))?;
        let mut file = File::create(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        fs::rename(&temporary, &self.path)
            .map_err(|error| format!("replace {}: {error}", self.path.display()))
    }
}

impl Drop for LockedState {
    fn drop(&mut self) {
        unsafe { libc::flock(self._lock.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn plugin_state_dir() -> PathBuf {
    env::var_os("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/state/herdr-dwm-layout"))
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| format!("response has no {name}: {value}"))
}

fn typed_field(result: Value, expected: &str, name: &str) -> Result<Value> {
    if result.get("type").and_then(Value::as_str) != Some(expected) {
        return Err(format!("expected {expected}, received {result}"));
    }
    field(&result, name).cloned()
}

fn current_pane(client: &mut impl Api) -> Result<Value> {
    let result = client.request("pane.current", json!({}))?;
    typed_field(result, "pane_current", "pane")
}

fn export_layout(client: &mut impl Api, tab_id: &str) -> Result<Value> {
    let result = client.request("layout.export", json!({"tab_id": tab_id}))?;
    typed_field(result, "layout_export", "layout")
}

fn pane_ids(node: &Value) -> Result<Vec<String>> {
    match node.get("type").and_then(Value::as_str) {
        Some("pane") => Ok(node
            .get("pane_id")
            .and_then(Value::as_str)
            .map(|id| vec![id.to_owned()])
            .unwrap_or_default()),
        Some("split") => {
            let mut ids = pane_ids(field(node, "first")?)?;
            ids.extend(pane_ids(field(node, "second")?)?);
            Ok(ids)
        }
        other => Err(format!("invalid layout node type {other:?}: {node}")),
    }
}

fn layout_pane_ids(layout: &Value) -> Result<Vec<String>> {
    pane_ids(field(layout, "root")?)
}

fn move_to_new_tab(client: &mut impl Api, pane_id: &str, workspace_id: &str) -> Result<String> {
    let result = client.request(
        "pane.move",
        json!({
            "pane_id": pane_id,
            "destination": {
                "type": "new_tab",
                "workspace_id": workspace_id,
                "label": "dwm-staging"
            },
            "focus": false
        }),
    )?;
    let moved = typed_field(result, "pane_move", "move_result")?;
    moved
        .pointer("/created_tab/tab_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Herdr did not create the DWM staging tab".to_owned())
}

fn move_to_tab(
    client: &mut impl Api,
    pane_id: &str,
    tab_id: &str,
    target_pane_id: &str,
    direction: &str,
    ratio: Option<f64>,
    focus: bool,
) -> Result<()> {
    let mut destination = json!({
        "type": "tab",
        "tab_id": tab_id,
        "split": direction,
        "target_pane_id": target_pane_id
    });
    if let Some(ratio) = ratio {
        destination["ratio"] = json!(ratio);
    }
    client.request(
        "pane.move",
        json!({"pane_id": pane_id, "destination": destination, "focus": focus}),
    )?;
    Ok(())
}

fn build_equal_group(
    client: &mut impl Api,
    tab_id: &str,
    group: &[String],
    inner_direction: &str,
    focused_pane_id: &str,
) -> Result<()> {
    let mut target = group
        .first()
        .ok_or_else(|| "empty pane group".to_owned())?
        .clone();
    let count = group.len();
    for (index, pane_id) in group.iter().enumerate().skip(1) {
        let remaining = count - index + 1;
        move_to_tab(
            client,
            pane_id,
            tab_id,
            &target,
            inner_direction,
            Some(1.0 / remaining as f64),
            pane_id == focused_pane_id,
        )?;
        target.clone_from(pane_id);
    }
    Ok(())
}

fn reflow(
    client: &mut impl Api,
    tab_id: &str,
    workspace_id: &str,
    tab_state: &mut TabState,
) -> Result<()> {
    let layout = export_layout(client, tab_id)?;
    let ordered = layout_pane_ids(&layout)?;
    if ordered.len() <= 1 {
        tab_state.pane_ids = ordered.clone();
        tab_state.nmaster = 1;
        tab_state.last_master_pane = ordered.first().cloned();
        tab_state.last_stack_pane = None;
        return Ok(());
    }

    let focused = field(&layout, "focused_pane_id")?
        .as_str()
        .ok_or_else(|| "focused_pane_id is not a string".to_owned())?
        .to_owned();
    let nmaster = tab_state.nmaster.clamp(1, ordered.len());
    tab_state.nmaster = nmaster;
    let (masters, stack) = ordered.split_at(nmaster);
    let (root_direction, inner_direction) = match tab_state.orientation {
        Orientation::Horizontal => ("right", "down"),
        Orientation::Vertical => ("down", "right"),
    };

    let staged = &ordered[1..];
    let staging_tab = move_to_new_tab(client, &staged[0], workspace_id)?;
    let mut staging_target = staged[0].clone();
    for pane_id in &staged[1..] {
        move_to_tab(
            client,
            pane_id,
            &staging_tab,
            &staging_target,
            "down",
            None,
            false,
        )?;
        staging_target.clone_from(pane_id);
    }

    if let Some(first_stack) = stack.first() {
        move_to_tab(
            client,
            first_stack,
            tab_id,
            &masters[0],
            root_direction,
            Some(tab_state.mfact),
            first_stack == &focused,
        )?;
    }
    build_equal_group(client, tab_id, masters, inner_direction, &focused)?;
    if !stack.is_empty() {
        build_equal_group(client, tab_id, stack, inner_direction, &focused)?;
    }
    if focused == masters[0] {
        client.request("pane.focus", json!({"pane_id": focused}))?;
    }

    tab_state.last_master_pane = masters.first().cloned();
    tab_state.last_stack_pane = stack.first().cloned();
    tab_state.pane_ids = ordered;
    Ok(())
}

fn context_ids(client: &mut impl Api) -> Result<(String, String, String, Value)> {
    let pane = current_pane(client)?;
    let get = |name: &str| -> Result<String> {
        pane.get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("current pane has no {name}"))
    };
    Ok((get("workspace_id")?, get("tab_id")?, get("pane_id")?, pane))
}

fn enable(client: &mut impl Api, state: &mut State) -> Result<()> {
    let (workspace_id, tab_id, _, _) = context_ids(client)?;
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    tab_state.enabled = true;
    reflow(client, &tab_id, &workspace_id, tab_state)
}

fn disable(client: &mut impl Api, state: &mut State) -> Result<()> {
    let (_, tab_id, _, _) = context_ids(client)?;
    state.tabs.remove(&tab_id);
    Ok(())
}

fn split_action(client: &mut impl Api, state: &mut State, direction: &str) -> Result<()> {
    let (workspace_id, tab_id, pane_id, pane) = context_ids(client)?;
    let cwd = pane
        .get("foreground_cwd")
        .or_else(|| pane.get("cwd"))
        .cloned()
        .unwrap_or(Value::Null);
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    let split_result = client.request(
        "pane.split",
        json!({
            "target_pane_id": pane_id,
            "direction": direction,
            "cwd": cwd,
            "focus": true
        }),
    )?;
    let new_pane = typed_field(split_result, "pane_info", "pane")?;
    let new_pane_id = field(&new_pane, "pane_id")?
        .as_str()
        .ok_or_else(|| "new pane has no pane_id".to_owned())?
        .to_owned();

    let ordered = layout_pane_ids(&export_layout(client, &tab_id)?)?;
    let nmaster = tab_state.nmaster.clamp(1, ordered.len());
    if !ordered[..nmaster].contains(&new_pane_id) {
        let target = tab_state
            .last_master_pane
            .as_ref()
            .filter(|id| ordered[..nmaster].contains(id))
            .cloned()
            .unwrap_or_else(|| ordered[0].clone());
        client.request(
            "pane.swap",
            json!({"source_pane_id": new_pane_id, "target_pane_id": target}),
        )?;
    }
    reflow(client, &tab_id, &workspace_id, tab_state)
}

fn swap_master(client: &mut impl Api, state: &mut State) -> Result<()> {
    let (workspace_id, tab_id, focused, _) = context_ids(client)?;
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    let ordered = layout_pane_ids(&export_layout(client, &tab_id)?)?;
    if ordered.len() < 2 {
        return Ok(());
    }
    let nmaster = tab_state.nmaster.clamp(1, ordered.len());
    let target = if ordered[..nmaster].contains(&focused) {
        tab_state
            .last_stack_pane
            .as_ref()
            .filter(|id| ordered[nmaster..].contains(id))
            .cloned()
            .unwrap_or_else(|| {
                ordered
                    .get(nmaster)
                    .unwrap_or(ordered.last().unwrap())
                    .clone()
            })
    } else {
        tab_state
            .last_master_pane
            .as_ref()
            .filter(|id| ordered[..nmaster].contains(id))
            .cloned()
            .unwrap_or_else(|| ordered[0].clone())
    };
    if target != focused {
        client.request(
            "pane.swap",
            json!({"source_pane_id": focused, "target_pane_id": target}),
        )?;
        reflow(client, &tab_id, &workspace_id, tab_state)?;
    }
    Ok(())
}

fn change_nmaster(client: &mut impl Api, state: &mut State, delta: i64) -> Result<()> {
    let (workspace_id, tab_id, _, _) = context_ids(client)?;
    let count = layout_pane_ids(&export_layout(client, &tab_id)?)?.len();
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    tab_state.nmaster = (tab_state.nmaster as i64 + delta).clamp(1, count as i64) as usize;
    reflow(client, &tab_id, &workspace_id, tab_state)
}

fn change_mfact(client: &mut impl Api, state: &mut State, delta: f64) -> Result<()> {
    let (workspace_id, tab_id, _, _) = context_ids(client)?;
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    tab_state.mfact =
        ((tab_state.mfact + delta).clamp(MIN_MFACT, MAX_MFACT) * 100.0).round() / 100.0;
    reflow(client, &tab_id, &workspace_id, tab_state)
}

fn next_layout(client: &mut impl Api, state: &mut State) -> Result<()> {
    let (workspace_id, tab_id, _, _) = context_ids(client)?;
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    tab_state.orientation = match tab_state.orientation {
        Orientation::Horizontal => Orientation::Vertical,
        Orientation::Vertical => Orientation::Horizontal,
    };
    reflow(client, &tab_id, &workspace_id, tab_state)
}

fn event_action(client: &mut impl Api, state: &mut State) -> Result<()> {
    let event: Value = env::var("HERDR_PLUGIN_EVENT_JSON")
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}));
    let data = event.get("data").unwrap_or(&event);
    let event_name = env::var("HERDR_PLUGIN_EVENT").unwrap_or_default();
    let event_type = data
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(&event_name)
        .replace('.', "_");
    if event_type == "tab_closed" {
        if let Some(tab_id) = data.get("tab_id").and_then(Value::as_str) {
            state.tabs.remove(tab_id);
        }
        return Ok(());
    }

    let pane = data.get("pane").unwrap_or(&Value::Null);
    let mut tab_id = pane
        .get("tab_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let workspace_id = pane
        .get("workspace_id")
        .or_else(|| data.get("workspace_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if tab_id.is_none() {
        if let Some(closed) = data.get("pane_id").and_then(Value::as_str) {
            let owners: Vec<_> = state
                .tabs
                .iter()
                .filter(|(_, tab)| tab.pane_ids.iter().any(|id| id == closed))
                .map(|(id, _)| id.clone())
                .collect();
            if owners.len() == 1 {
                tab_id = owners.first().cloned();
            }
        }
    }
    let (Some(tab_id), Some(workspace_id)) = (tab_id, workspace_id) else {
        return Ok(());
    };
    let Some(tab_state) = state.tabs.get_mut(&tab_id) else {
        return Ok(());
    };
    if !tab_state.enabled {
        return Ok(());
    }
    let current_ids = layout_pane_ids(&export_layout(client, &tab_id)?)?;
    if current_ids == tab_state.pane_ids {
        return Ok(());
    }
    reflow(client, &tab_id, &workspace_id, tab_state)
}

fn reflow_target(client: &mut impl Api, state: &mut State, arguments: &[String]) -> Result<()> {
    let workspace_id = arguments
        .get(2)
        .ok_or_else(|| "reflow-tab requires workspace id".to_owned())?;
    let tab_id = arguments
        .get(3)
        .ok_or_else(|| "reflow-tab requires tab id".to_owned())?;
    let orientation = match arguments.get(4).map(String::as_str).unwrap_or("horizontal") {
        "horizontal" => Orientation::Horizontal,
        "vertical" => Orientation::Vertical,
        value => return Err(format!("invalid orientation: {value}")),
    };
    let mfact = arguments
        .get(5)
        .map(String::as_str)
        .unwrap_or("0.5")
        .parse()
        .map_err(|error| format!("invalid mfact: {error}"))?;
    let nmaster = arguments
        .get(6)
        .map(String::as_str)
        .unwrap_or("1")
        .parse()
        .map_err(|error| format!("invalid nmaster: {error}"))?;
    let tab_state = state.tabs.entry(tab_id.clone()).or_default();
    tab_state.orientation = orientation;
    tab_state.mfact = mfact;
    tab_state.nmaster = nmaster;
    reflow(client, tab_id, workspace_id, tab_state)
}

fn run() -> Result<()> {
    let arguments: Vec<String> = env::args().collect();
    let action = arguments
        .get(1)
        .ok_or_else(|| "missing action".to_owned())?;
    let mut client = HerdrClient::from_environment();
    let mut locked = LockedState::open()?;
    match action.as_str() {
        "enable" => enable(&mut client, &mut locked.state)?,
        "disable" => disable(&mut client, &mut locked.state)?,
        "split" => split_action(
            &mut client,
            &mut locked.state,
            arguments
                .get(2)
                .ok_or_else(|| "split requires direction".to_owned())?,
        )?,
        "swap-master" => swap_master(&mut client, &mut locked.state)?,
        "next-layout" => next_layout(&mut client, &mut locked.state)?,
        "change-nmaster" => change_nmaster(
            &mut client,
            &mut locked.state,
            arguments
                .get(2)
                .ok_or_else(|| "change-nmaster requires delta".to_owned())?
                .parse()
                .map_err(|error| format!("invalid nmaster delta: {error}"))?,
        )?,
        "change-mfact" => change_mfact(
            &mut client,
            &mut locked.state,
            arguments
                .get(2)
                .ok_or_else(|| "change-mfact requires delta".to_owned())?
                .parse()
                .map_err(|error| format!("invalid mfact delta: {error}"))?,
        )?,
        "event" => event_action(&mut client, &mut locked.state)?,
        "reflow-tab" => reflow_target(&mut client, &mut locked.state, &arguments)?,
        unknown => return Err(format!("unknown action: {unknown}")),
    }
    locked.save()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("dwm-layout: {error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct MockApi {
        responses: VecDeque<Value>,
        calls: Vec<(String, Value)>,
    }

    impl MockApi {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: responses.into(),
                calls: Vec::new(),
            }
        }
    }

    impl Api for MockApi {
        fn request(&mut self, method: &str, params: Value) -> Result<Value> {
            self.calls.push((method.to_owned(), params));
            self.responses
                .pop_front()
                .ok_or_else(|| format!("no response for {method}"))
        }
    }

    fn pane(id: &str) -> Value {
        json!({"type": "pane", "pane_id": id})
    }
    fn split(direction: &str, first: Value, second: Value) -> Value {
        json!({"type": "split", "direction": direction, "ratio": 0.5, "first": first, "second": second})
    }
    fn layout(root: Value, focused: &str) -> Value {
        json!({"type": "layout_export", "layout": {"workspace_id": "w1", "tab_id": "w1:t1", "zoomed": false, "focused_pane_id": focused, "root": root}})
    }
    fn moved(stage: Option<&str>) -> Value {
        json!({"type": "pane_move", "move_result": {"created_tab": stage.map(|id| json!({"tab_id": id}))}})
    }

    #[test]
    fn bsp_order_is_stable() {
        let root = split(
            "right",
            split("down", pane("p1"), pane("p2")),
            split("down", pane("p3"), pane("p4")),
        );
        assert_eq!(pane_ids(&root).unwrap(), ["p1", "p2", "p3", "p4"]);
    }

    #[test]
    fn single_pane_updates_state_without_moves() {
        let mut api = MockApi::new(vec![layout(pane("w1:p1"), "w1:p1")]);
        let mut state = TabState {
            nmaster: 4,
            ..Default::default()
        };
        reflow(&mut api, "w1:t1", "w1", &mut state).unwrap();
        assert_eq!(state.nmaster, 1);
        assert_eq!(state.pane_ids, ["w1:p1"]);
        assert_eq!(api.calls.len(), 1);
    }

    #[test]
    fn horizontal_reflow_builds_master_boundary_first() {
        let root = split(
            "down",
            pane("w1:p1"),
            split("down", pane("w1:p2"), pane("w1:p3")),
        );
        let mut api = MockApi::new(vec![
            layout(root, "w1:p1"),
            moved(Some("w1:stage")),
            moved(None),
            moved(None),
            moved(None),
            json!({"type": "pane_focus"}),
        ]);
        let mut state = TabState {
            mfact: 0.6,
            ..Default::default()
        };
        reflow(&mut api, "w1:t1", "w1", &mut state).unwrap();
        let source_move = api
            .calls
            .iter()
            .find(|(method, params)| {
                method == "pane.move"
                    && params
                        .pointer("/destination/tab_id")
                        .and_then(Value::as_str)
                        == Some("w1:t1")
            })
            .unwrap();
        assert_eq!(source_move.1["pane_id"], "w1:p2");
        assert_eq!(source_move.1["destination"]["split"], "right");
        assert_eq!(source_move.1["destination"]["ratio"], 0.6);
    }

    #[test]
    fn a_new_split_is_promoted_to_master() {
        let current = json!({
            "type": "pane_current",
            "pane": {
                "workspace_id": "w1",
                "tab_id": "w1:t1",
                "pane_id": "w1:p1",
                "cwd": "/tmp",
                "foreground_cwd": "/tmp"
            }
        });
        let created = json!({
            "type": "pane_info",
            "pane": {"pane_id": "w1:p2"}
        });
        let before_swap = layout(split("down", pane("w1:p1"), pane("w1:p2")), "w1:p2");
        let after_swap = layout(split("down", pane("w1:p2"), pane("w1:p1")), "w1:p2");
        let mut api = MockApi::new(vec![
            current,
            created,
            before_swap,
            json!({}),
            after_swap,
            moved(Some("w1:stage")),
            moved(None),
            json!({"type": "pane_focus"}),
        ]);
        let mut state = State::default();

        split_action(&mut api, &mut state, "down").unwrap();

        let swap = api
            .calls
            .iter()
            .find(|(method, _)| method == "pane.swap")
            .unwrap();
        assert_eq!(swap.1["source_pane_id"], "w1:p2");
        assert_eq!(swap.1["target_pane_id"], "w1:p1");
        assert_eq!(state.tabs["w1:t1"].pane_ids, ["w1:p2", "w1:p1"]);
    }

    #[test]
    fn old_python_state_deserializes_with_new_fields() {
        let state: State = serde_json::from_value(json!({"version": 1, "tabs": {"w1:t1": {"enabled": true, "orientation": "vertical", "mfact": 0.7, "nmaster": 2, "last_master_pane": null, "last_stack_pane": null}}})).unwrap();
        let tab = &state.tabs["w1:t1"];
        assert_eq!(tab.orientation, Orientation::Vertical);
        assert!(tab.pane_ids.is_empty());
    }
}
