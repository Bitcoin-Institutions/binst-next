//! Application state and its pure update function.
//!
//! `update` never touches the browser: it changes the state and returns the
//! effects (network calls, wallet prompts, storage writes) for `store` to run.
//! That keeps every transition testable with `cargo test`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::{
    Definition, Instance, Layer, Load, NodeRef, Provenance, Source, Stage, Step, Tree, UserError,
};
use crate::nostr::Membership;
use crate::router::Route;

// ── Wallet sessions ──────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BtcSession {
    pub address: String,
    /// Public key as the wallet returned it (compressed or x-only hex).
    pub pubkey: String,
    /// `window` path of the provider, e.g. "unisat".
    pub provider_key: String,
    /// Separate funding address, when the wallet has one (Xverse).
    #[serde(default)]
    pub payment_address: String,
    pub wallet_name: String,
    /// "" for a browser extension; "handoff" for a watch-only account whose
    /// PSBTs are signed elsewhere (file, QR, Bitcoin Core).
    #[serde(default)]
    pub signer: String,
    /// Key origin of `pubkey` for hand-off signers: `fp/86'/1'/0'/0/i`.
    #[serde(default)]
    pub origin: String,
    /// The imported account descriptor (hand-off signers).
    #[serde(default)]
    pub account: String,
}

impl BtcSession {
    pub fn is_handoff(&self) -> bool {
        self.signer == "handoff"
    }
}

/// A PSBT waiting for a signer outside the browser.
#[derive(Clone, Debug, PartialEq)]
pub struct Handoff {
    /// "commit" or "reveal".
    pub step: String,
    /// What is being inscribed, for the dialog title.
    pub label: String,
    pub psbt_b64: String,
    pub format: crate::qr::QrFormat,
    /// Why the last thing handed back was refused.
    pub error: Option<String>,
    /// What the scanner says when it starts; `None` when the camera is off.
    /// Later progress is written straight into the dialog, so the video
    /// element is not re-created mid-scan.
    pub scanning: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvmSession {
    pub address: String,
    pub wallet_name: String,
    /// "citrea-testnet" when the wallet is on the right chain.
    pub network: String,
    /// EIP-6963 reverse-DNS id, used to find the same wallet after a reload.
    #[serde(default)]
    pub rdns: String,
}

// ── Local registry ───────────────────────────────────────────────

/// Things this browser broadcast, kept so they show up before the indexer
/// has them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub institutions: Vec<LocalInstitution>,
    #[serde(default)]
    pub definitions: Vec<LocalDefinition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalInstitution {
    pub id: String,
    pub reveal_txid: String,
    pub name: String,
    pub description: String,
    pub website: String,
    pub admin: String,
    pub holder: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalDefinition {
    pub id: String,
    pub reveal_txid: String,
    pub institution_id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
}

// ── Transient UI state ───────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u32,
    pub kind: ToastKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CreateStep {
    #[default]
    Details,
    Review,
    Signing,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateState {
    pub step: CreateStep,
    pub name: String,
    pub description: String,
    pub website: String,
    /// sat/vB; 0 until the recommended rate arrives.
    pub fee_rate: u64,
    pub progress: String,
    pub error: Option<String>,
    pub result_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefineDraft {
    pub institution: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    pub busy: bool,
    pub progress: String,
    pub error: Option<String>,
}

/// A transaction in flight, shown until it settles.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub id: u32,
    pub label: String,
    pub txid: String,
    pub layer: Layer,
    pub stage: OpStage,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpStage {
    Mempool,
    Confirmed(u32),
    Failed(String),
}

/// What the relays hold about an institution's members, after verification.
#[derive(Clone, Debug, PartialEq)]
pub struct MembersView {
    /// `None` when no announcement with a valid binding was found.
    pub membership: Option<Membership>,
    /// Why announcements that were found were ignored.
    pub rejected: Vec<String>,
    pub asked: usize,
    pub answered: usize,
}

/// Which instance action is waiting on the wallet or the chain.
#[derive(Clone, Debug, PartialEq)]
pub enum Busy {
    StartingInstance(String),
    ExecutingStep(String),
}

// ── State ────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub route: Route,
    pub btc: Option<BtcSession>,
    pub evm: Option<EvmSession>,
    /// Payment address reported during a connect that has not finished yet.
    pub pending_payment_address: String,
    pub registry: Registry,
    /// The institution shown to visitors without a wallet.
    pub featured: String,
    pub trees: HashMap<String, Load<Tree>>,
    /// Institutions held by the connected wallet or created in this browser.
    pub mine: Load<Vec<String>>,
    pub selected: Option<NodeRef>,
    pub picker: Option<Layer>,
    pub list_view: bool,
    /// Tree zoom in percent.
    pub zoom: u32,
    pub create: CreateState,
    pub define: Option<DefineDraft>,
    pub busy: Option<Busy>,
    /// Members and roles per institution, read from Nostr relays.
    pub members: HashMap<String, Load<MembersView>>,
    /// The members action in progress, in words.
    pub members_busy: Option<String>,
    pub members_error: Option<String>,
    /// Institutions announced on Nostr relays: `(id, announced name)`.
    pub announced: Load<Vec<(String, String)>>,
    pub ops: Vec<Op>,
    /// Chain data sources (user's own servers or the defaults).
    pub settings: crate::settings::Settings,
    /// Why the last account import failed.
    pub import_error: Option<String>,
    /// The PSBT hand-off dialog, while a signer outside the browser is needed.
    pub handoff: Option<Handoff>,
    pub toasts: Vec<Toast>,
    /// Toasts raised during the current update, waiting for their expiry effect.
    new_toasts: Vec<u32>,
    next_id: u32,
}

impl AppState {
    pub fn new(featured: &str) -> Self {
        AppState {
            route: Route::Home,
            btc: None,
            evm: None,
            pending_payment_address: String::new(),
            registry: Registry::default(),
            featured: featured.to_string(),
            trees: HashMap::new(),
            mine: Load::Idle,
            selected: None,
            picker: None,
            list_view: false,
            zoom: 100,
            create: CreateState::default(),
            define: None,
            busy: None,
            members: HashMap::new(),
            members_busy: None,
            members_error: None,
            announced: Load::Idle,
            ops: Vec::new(),
            settings: crate::settings::Settings::default(),
            import_error: None,
            handoff: None,
            toasts: Vec::new(),
            new_toasts: Vec::new(),
            next_id: 1,
        }
    }

    fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn tree(&self, id: &str) -> Option<&Tree> {
        match self.trees.get(id) {
            Some(Load::Ready(tree)) => Some(tree),
            _ => None,
        }
    }

    fn tree_mut(&mut self, id: &str) -> Option<&mut Tree> {
        match self.trees.get_mut(id) {
            Some(Load::Ready(tree)) => Some(tree),
            _ => None,
        }
    }

    /// True when the connected Bitcoin wallet holds this institution.
    pub fn holds(&self, tree: &Tree) -> bool {
        self.btc.as_ref().is_some_and(|b| !tree.institution.holder.is_empty() && b.address == tree.institution.holder)
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let id = self.fresh_id();
        self.toasts.push(Toast { id, kind, text: text.into() });
        self.new_toasts.push(id);
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    /// Ask the relays for an institution's members once its tree (and so its
    /// holder) is known.
    fn want_members(&mut self, id: &str, effects: &mut Vec<Effect>) {
        let known = matches!(self.members.get(id), Some(Load::Loading | Load::Ready(_)));
        if self.tree(id).is_some() && !known {
            self.members.insert(id.to_string(), Load::Loading);
            effects.push(Effect::LoadMembers(id.to_string()));
        }
    }

    fn bound_issuer(&self, id: &str) -> Option<String> {
        match self.members.get(id) {
            Some(Load::Ready(view)) => view.membership.as_ref().map(|m| m.binding.issuer.clone()),
            _ => None,
        }
    }

    /// Mark a tree as loading unless it is already there, and ask for it.
    fn want_tree(&mut self, id: &str, effects: &mut Vec<Effect>) {
        if !matches!(self.trees.get(id), Some(Load::Loading | Load::Ready(_))) {
            self.trees.insert(id.to_string(), Load::Loading);
            effects.push(Effect::LoadTree(id.to_string()));
        }
    }
}

// ── Messages and effects ─────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Navigate(Route),

    OpenPicker(Layer),
    ClosePicker,
    ConnectBtc { key: String, name: String },
    ConnectEvm { index: usize },
    ConnectWalletConnect,
    BtcConnected(BtcSession),
    EvmConnected(EvmSession),
    ConnectFailed(String),
    /// Use a watch-only Taproot account (descriptor) whose PSBTs are signed elsewhere.
    ImportAccount { descriptor: String, index: u32 },
    Disconnect(Layer),
    SetPaymentAddress(String),
    RegistryRestored(Registry),
    /// Sessions read back from storage on load (no toast, no prompt).
    SessionsRestored { btc: Option<BtcSession>, evm: Option<EvmSession> },

    ReloadTree(String),
    TreeLoaded(String, Result<Tree, UserError>),
    DefinitionsLoaded(String, Result<Vec<Definition>, String>),
    InstancesLoaded { institution: String, definition: String, result: Result<Vec<Instance>, String> },
    ReloadMine,
    MineLoaded(Result<Vec<String>, UserError>),
    InstanceResolved { address: String, result: Result<(String, String), UserError> },

    Select(Option<NodeRef>),
    ListView(bool),
    Zoom(i32),

    Toast(ToastKind, String),
    DismissToast(u32),

    CreateReview { name: String, description: String, website: String },
    CreateBack,
    CreateSubmit,
    CreateProgress(String),
    CreateDone(LocalInstitution),
    CreateFailed(String),
    CreateReset,
    FeeRate(u64),

    DefineOpen(String),
    DefineClose,
    DefineEdit { name: String, description: String, steps: Vec<Step>, then: DefineThen },
    DefineProgress(String),
    DefineDone(LocalDefinition),
    DefineFailed(String),

    StartInstance { institution: String, definition: String },
    InstanceStarted { institution: String, definition: String, result: Result<(String, String), String> },
    ExecuteStep { institution: String, definition: String, address: String },
    StepExecuted { institution: String, definition: String, address: String, result: Result<String, String> },

    OpUpdate { id: u32, stage: OpStage },

    ReloadMembers(String),
    MembersLoaded(String, Result<MembersView, UserError>),
    BindNostrKey(String),
    DefineRole { institution: String, name: String, description: String },
    AwardRole { institution: String, slug: String, member: String },
    SetRevoked { institution: String, slug: String, member: String, revoked: bool },
    MembersProgress(String),
    MembersDone { institution: String, text: String },
    MembersFailed(String),
    ReloadAnnounced,
    AnnouncedLoaded(Result<Vec<(String, String)>, UserError>),

    HandoffOpen { step: String, label: String, psbt_b64: String },
    /// A signed PSBT or transaction handed back (pasted, loaded or scanned).
    HandoffSubmit(Vec<u8>),
    HandoffCancel,
    HandoffFormat(crate::qr::QrFormat),
    HandoffCamera(bool),

    SettingsRestored(crate::settings::Settings),
    SaveSettings(crate::settings::Settings),
    ForgetRecovery(String),
}

/// What to do after the define form's fields were read.
#[derive(Clone, Debug, PartialEq)]
pub enum DefineThen {
    AddStep,
    RemoveStep(usize),
    Submit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Change the URL without navigating.
    ReplaceHash(String),
    LoadTree(String),
    LoadDefinitions(String),
    LoadInstances { institution: String, definition: String },
    LoadMine,
    ResolveInstance(String),
    ConnectBtc { key: String, name: String },
    ConnectEvm { index: usize },
    ConnectWalletConnect,
    ForgetEvmProvider,
    SaveSessions,
    SaveRegistry,
    FetchFeeRate,
    InscribeInstitution { name: String, description: String, website: String, fee_rate: u64 },
    InscribeDefinition { institution: String, name: String, description: String, steps: Vec<Step>, fee_rate: u64 },
    StartInstance { institution: String, definition: String, steps: Vec<Step> },
    ExecuteStep { institution: String, definition: String, address: String, step: u32 },
    PollConfirmation { op: u32, txid: String },
    LoadMembers(String),
    LoadAnnounced,
    BindNostrKey { institution: String, name: String, holder: String },
    PublishRole { institution: String, issuer: String, slug: String, name: String, description: String },
    PublishAward { institution: String, issuer: String, slug: String, member: String },
    PublishRevocations { institution: String, issuer: String, revoked: Vec<(String, String)> },
    ExpireToast(u32),
    HandoffResolve(Vec<u8>),
    HandoffReject,
    StartCamera,
    StopCamera,
    SaveSettings,
    ForgetRecovery(String),
}

const MAX_NAME: usize = 80;
const MAX_STEPS: usize = 12;

/// Apply a message. Returns the effects to run.
pub fn update(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    let mut fx = Vec::new();
    match msg {
        Msg::Navigate(route) => {
            state.picker = None;
            state.selected = None;
            state.define = None;
            match &route {
                Route::Home => {
                    let featured = state.featured.clone();
                    state.want_tree(&featured, &mut fx);
                }
                Route::Institutions => {
                    let featured = state.featured.clone();
                    state.want_tree(&featured, &mut fx);
                    if !matches!(state.mine, Load::Loading | Load::Ready(_)) {
                        state.mine = Load::Loading;
                        fx.push(Effect::LoadMine);
                    }
                }
                Route::Institution { id, definition } => {
                    state.want_tree(id, &mut fx);
                    if let Some(def) = definition {
                        state.selected = Some(NodeRef::Definition { institution: id.clone(), id: def.clone() });
                    }
                }
                Route::Members { id } => {
                    state.want_tree(id, &mut fx);
                    state.want_members(id, &mut fx);
                    state.members_error = None;
                }
                Route::Instance { address } => fx.push(Effect::ResolveInstance(address.clone())),
                Route::Settings => {}
                Route::New => {
                    if state.create.step == CreateStep::Done {
                        state.create = CreateState::default();
                    }
                    if state.create.fee_rate == 0 {
                        fx.push(Effect::FetchFeeRate);
                    }
                }
                Route::Lab(Some(page)) if page == "nostr" && !matches!(state.announced, Load::Loading | Load::Ready(_)) => {
                    state.announced = Load::Loading;
                    fx.push(Effect::LoadAnnounced);
                }
                Route::Lab(_) | Route::NotFound(_) => {}
            }
            state.route = route;
        }

        // ── Wallets ──────────────────────────────────────────────
        Msg::OpenPicker(layer) => state.picker = Some(layer),
        Msg::ClosePicker => state.picker = None,
        Msg::ConnectBtc { key, name } => {
            state.pending_payment_address.clear();
            fx.push(Effect::ConnectBtc { key, name });
        }
        Msg::ConnectEvm { index } => fx.push(Effect::ConnectEvm { index }),
        Msg::ConnectWalletConnect => fx.push(Effect::ConnectWalletConnect),
        Msg::BtcConnected(mut session) => {
            if session.payment_address.is_empty() {
                session.payment_address = std::mem::take(&mut state.pending_payment_address);
            }
            let changed = state.btc.as_ref().map(|b| b.address.as_str()) != Some(session.address.as_str());
            state.toast(ToastKind::Success, format!("{} connected on Bitcoin testnet4", session.wallet_name));
            state.btc = Some(session);
            state.picker = None;
            fx.push(Effect::SaveSessions);
            if changed {
                state.mine = Load::Loading;
                fx.push(Effect::LoadMine);
            }
        }
        Msg::EvmConnected(session) => {
            if session.network == "citrea-testnet" || session.network == "hardhat-local" {
                state.toast(ToastKind::Success, format!("{} connected on Citrea testnet", session.wallet_name));
            } else {
                state.toast(
                    ToastKind::Error,
                    format!("{} is on another network ({}). Switch it to Citrea testnet.", session.wallet_name, session.network),
                );
            }
            state.evm = Some(session);
            state.picker = None;
            fx.push(Effect::SaveSessions);
        }
        Msg::ConnectFailed(why) => state.toast(ToastKind::Error, why),
        Msg::ImportAccount { descriptor, index } => match crate::account::parse(&descriptor, index) {
            Ok(account) => {
                state.import_error = None;
                let fp = account.origin.as_ref().map(|(fp, _)| fp.to_string());
                let session = BtcSession {
                    address: account.address,
                    pubkey: account.internal.to_string(),
                    provider_key: "handoff".to_string(),
                    payment_address: String::new(),
                    wallet_name: match fp {
                        Some(fp) => format!("PSBT signer [{fp}]"),
                        None => "PSBT signer".to_string(),
                    },
                    signer: "handoff".to_string(),
                    origin: account.origin.as_ref().map(crate::account::origin_to_string).unwrap_or_default(),
                    account: account.source,
                };
                if session.origin.is_empty() {
                    state.toast(ToastKind::Info, "No key origin in that descriptor: Sparrow and Bitcoin Core can still sign, hardware signers may not");
                }
                fx.extend(update(state, Msg::BtcConnected(session)));
            }
            Err(why) => state.import_error = Some(why),
        },
        Msg::Disconnect(Layer::Bitcoin) => {
            state.btc = None;
            state.mine = Load::Idle;
            fx.push(Effect::SaveSessions);
            if state.route == Route::Institutions {
                state.mine = Load::Loading;
                fx.push(Effect::LoadMine);
            }
        }
        Msg::Disconnect(Layer::Citrea) => {
            state.evm = None;
            fx.push(Effect::ForgetEvmProvider);
            fx.push(Effect::SaveSessions);
        }
        Msg::SetPaymentAddress(address) => {
            if let Some(btc) = state.btc.as_mut() {
                btc.payment_address = address.clone();
            }
            state.pending_payment_address = address;
        }
        Msg::RegistryRestored(registry) => state.registry = registry,
        Msg::SessionsRestored { btc, evm } => {
            state.btc = btc;
            state.evm = evm;
        }


        // ── PSBT hand-off ────────────────────────────────────────
        Msg::HandoffOpen { step, label, psbt_b64 } => {
            state.handoff = Some(Handoff {
                step,
                label,
                psbt_b64,
                format: crate::qr::QrFormat::Ur,
                error: None,
                scanning: None,
            });
        }
        Msg::HandoffSubmit(bytes) => {
            if let Some(h) = state.handoff.as_mut() {
                let original = bitcoin::base64::Engine::decode(&bitcoin::base64::engine::general_purpose::STANDARD, &h.psbt_b64)
                    .ok()
                    .and_then(|b| bitcoin::psbt::Psbt::deserialize(&b).ok());
                let checked = original
                    .ok_or_else(|| "The PSBT sent out could not be read back".to_string())
                    .and_then(|orig| crate::reveal::merge_signed(&orig, &bytes).map(|m| (orig, m)));
                match checked {
                    Ok((_, mut merged)) => {
                        crate::reveal::finalize_key_path_inputs(&mut merged);
                        let missing = crate::reveal::unsigned_inputs(&merged);
                        let verified = if missing.is_empty() { crate::reveal::verify_key_path_signatures(&merged) } else { Ok(()) };
                        if let Err(why) = verified {
                            h.error = Some(why);
                        } else if missing.is_empty() {
                            let was_scanning = h.scanning.is_some();
                            state.handoff = None;
                            if was_scanning {
                                fx.push(Effect::StopCamera);
                            }
                            fx.push(Effect::HandoffResolve(bytes));
                        } else {
                            h.error = Some(format!(
                                "That PSBT is not signed yet (input {missing:?} has no signature). Sign it, then hand back the signed file."
                            ));
                        }
                    }
                    Err(why) => h.error = Some(why),
                }
            }
        }
        Msg::HandoffCancel => {
            if let Some(h) = state.handoff.take() {
                if h.scanning.is_some() {
                    fx.push(Effect::StopCamera);
                }
                fx.push(Effect::HandoffReject);
            }
        }
        Msg::HandoffFormat(format) => {
            if let Some(h) = state.handoff.as_mut() {
                h.format = format;
            }
        }
        Msg::HandoffCamera(on) => {
            if let Some(h) = state.handoff.as_mut() {
                if on && h.scanning.is_none() {
                    h.scanning = Some("Starting the camera…".to_string());
                    fx.push(Effect::StartCamera);
                } else if !on && h.scanning.is_some() {
                    h.scanning = None;
                    fx.push(Effect::StopCamera);
                }
            }
        }
        // ── Settings ─────────────────────────────────────────────
        Msg::SettingsRestored(settings) => state.settings = settings,
        Msg::SaveSettings(settings) => match settings.normalized() {
            Ok(settings) => {
                let indexer_changed = settings.ord_url != state.settings.ord_url;
                state.settings = settings;
                fx.push(Effect::SaveSettings);
                state.toast(ToastKind::Success, "Settings saved");
                if indexer_changed {
                    state.trees.clear();
                    state.mine = Load::Idle;
                }
            }
            Err(why) => state.toast(ToastKind::Error, why),
        },
        Msg::ForgetRecovery(commit_txid) => fx.push(Effect::ForgetRecovery(commit_txid)),

        // ── Trees ────────────────────────────────────────────────
        Msg::ReloadTree(id) => {
            state.trees.insert(id.clone(), Load::Loading);
            fx.push(Effect::LoadTree(id));
        }
        Msg::TreeLoaded(id, Ok(mut tree)) => {
            let follow_up = tree.definitions_stage == Stage::Loading;
            merge_pending_definitions(&state.registry, &mut tree);
            state.trees.insert(id.clone(), Load::Ready(tree));
            if state.route == (Route::Members { id: id.clone() }) {
                state.want_members(&id, &mut fx);
            }
            if follow_up {
                fx.push(Effect::LoadDefinitions(id));
            }
        }
        Msg::TreeLoaded(id, Err(error)) => {
            state.trees.insert(id, Load::Failed(error));
        }
        Msg::DefinitionsLoaded(id, result) => {
            let registry = state.registry.clone();
            if let Some(tree) = state.tree_mut(&id) {
                match result {
                    Ok(definitions) => {
                        tree.definitions = definitions;
                        tree.definitions_stage = Stage::Ready;
                        merge_pending_definitions(&registry, tree);
                        for def in tree.definitions.iter().filter(|d| d.provenance != Provenance::Pending) {
                            fx.push(Effect::LoadInstances { institution: id.clone(), definition: def.id.clone() });
                        }
                    }
                    Err(why) => tree.definitions_stage = Stage::Failed(why),
                }
            }
        }
        Msg::InstancesLoaded { institution, definition, result } => {
            if let Some(def) = state.tree_mut(&institution).and_then(|t| t.definition_mut(&definition)) {
                match result {
                    Ok(instances) => {
                        def.instances = instances;
                        def.instances_stage = Stage::Ready;
                    }
                    Err(why) => def.instances_stage = Stage::Failed(why),
                }
            }
        }
        Msg::ReloadMine => {
            state.mine = Load::Loading;
            fx.push(Effect::LoadMine);
        }
        Msg::MineLoaded(Ok(ids)) => {
            for id in &ids {
                state.want_tree(id, &mut fx);
            }
            state.mine = Load::Ready(ids);
        }
        Msg::MineLoaded(Err(error)) => state.mine = Load::Failed(error),
        Msg::InstanceResolved { address, result } => {
            if state.route == (Route::Instance { address: address.clone() }) {
                match result {
                    Ok((institution, _definition)) => {
                        state.want_tree(&institution, &mut fx);
                        state.route = Route::Institution { id: institution.clone(), definition: None };
                        state.selected = Some(NodeRef::Instance { institution, address });
                    }
                    Err(error) => state.toast(ToastKind::Error, format!("{}: {}", error.what, error.why)),
                }
            }
        }

        Msg::Select(node) => {
            state.define = None;
            if let Route::Institution { id, .. } = &state.route {
                let definition = match &node {
                    Some(NodeRef::Definition { id: def, .. }) => Some(def.clone()),
                    _ => None,
                };
                let route = Route::Institution { id: id.clone(), definition };
                fx.push(Effect::ReplaceHash(crate::router::href(&route)));
                state.route = route;
            }
            state.selected = node;
        }
        Msg::ListView(on) => state.list_view = on,
        Msg::Zoom(delta) => {
            state.zoom = if delta == 0 { 100 } else { (state.zoom as i32 + delta).clamp(40, 160) as u32 };
        }

        Msg::Toast(kind, text) => state.toast(kind, text),
        Msg::DismissToast(id) => state.toasts.retain(|t| t.id != id),

        // ── Create institution ───────────────────────────────────
        Msg::CreateReview { name, description, website } => {
            let name = name.trim().to_string();
            state.create.description = description.trim().to_string();
            state.create.website = website.trim().to_string();
            state.create.error = if name.is_empty() {
                Some("Give the institution a name.".to_string())
            } else if name.chars().count() > MAX_NAME {
                Some(format!("Keep the name under {MAX_NAME} characters."))
            } else if !state.create.website.is_empty() && !state.create.website.starts_with("https://") {
                Some("The website must start with https://".to_string())
            } else {
                None
            };
            state.create.name = name;
            if state.create.error.is_none() {
                state.create.step = CreateStep::Review;
                if state.create.fee_rate == 0 {
                    fx.push(Effect::FetchFeeRate);
                }
            }
        }
        Msg::CreateBack => {
            state.create.step = CreateStep::Details;
            state.create.error = None;
        }
        Msg::CreateSubmit => {
            if state.btc.is_none() {
                state.picker = Some(Layer::Bitcoin);
            } else if state.create.step == CreateStep::Review {
                state.create.step = CreateStep::Signing;
                state.create.error = None;
                state.create.progress = "Preparing the commit and reveal transactions…".to_string();
                fx.push(Effect::InscribeInstitution {
                    name: state.create.name.clone(),
                    description: state.create.description.clone(),
                    website: state.create.website.clone(),
                    fee_rate: state.create.fee_rate,
                });
            }
        }
        Msg::CreateProgress(text) => state.create.progress = text,
        Msg::CreateDone(local) => {
            let op = state.fresh_id();
            state.ops.push(Op {
                id: op,
                label: format!("Institution “{}”", local.name),
                txid: local.reveal_txid.clone(),
                layer: Layer::Bitcoin,
                stage: OpStage::Mempool,
            });
            fx.push(Effect::PollConfirmation { op, txid: local.reveal_txid.clone() });
            state.create.step = CreateStep::Done;
            state.create.result_id = Some(local.id.clone());
            state.trees.remove(&local.id);
            if let Load::Ready(ids) = &mut state.mine {
                ids.insert(0, local.id.clone());
            }
            state.registry.institutions.retain(|i| i.id != local.id);
            state.registry.institutions.insert(0, local);
            fx.push(Effect::SaveRegistry);
        }
        Msg::CreateFailed(why) => {
            state.create.step = CreateStep::Review;
            state.create.error = Some(why);
        }
        Msg::CreateReset => state.create = CreateState { fee_rate: state.create.fee_rate, ..CreateState::default() },
        Msg::FeeRate(rate) => state.create.fee_rate = rate.clamp(1, 50),

        // ── Define process ───────────────────────────────────────
        Msg::DefineOpen(institution) => {
            if state.btc.is_none() {
                state.picker = Some(Layer::Bitcoin);
            } else {
                state.define = Some(DefineDraft {
                    institution,
                    steps: vec![Step { name: String::new(), action: "approval".to_string() }],
                    ..DefineDraft::default()
                });
                if state.create.fee_rate == 0 {
                    fx.push(Effect::FetchFeeRate);
                }
            }
        }
        Msg::DefineClose => state.define = None,
        Msg::DefineEdit { name, description, steps, then } => {
            let fee_rate = state.create.fee_rate;
            if let Some(draft) = state.define.as_mut() {
                draft.name = name.trim().to_string();
                draft.description = description.trim().to_string();
                draft.steps = steps;
                draft.error = None;
                match then {
                    DefineThen::AddStep => {
                        if draft.steps.len() < MAX_STEPS {
                            draft.steps.push(Step { name: String::new(), action: "approval".to_string() });
                        }
                    }
                    DefineThen::RemoveStep(index) => {
                        if draft.steps.len() > 1 && index < draft.steps.len() {
                            draft.steps.remove(index);
                        }
                    }
                    DefineThen::Submit => {
                        for step in &mut draft.steps {
                            step.name = step.name.trim().to_string();
                        }
                        if draft.name.is_empty() {
                            draft.error = Some("Give the process a name.".to_string());
                        } else if draft.steps.iter().any(|s| s.name.is_empty()) {
                            draft.error = Some("Every step needs a name.".to_string());
                        } else {
                            draft.busy = true;
                            draft.progress = "Locating the institution's UTXO…".to_string();
                            fx.push(Effect::InscribeDefinition {
                                institution: draft.institution.clone(),
                                name: draft.name.clone(),
                                description: draft.description.clone(),
                                steps: draft.steps.clone(),
                                fee_rate,
                            });
                        }
                    }
                }
            }
        }
        Msg::DefineProgress(text) => {
            if let Some(draft) = state.define.as_mut() {
                draft.progress = text;
            }
        }
        Msg::DefineDone(local) => {
            state.define = None;
            let op = state.fresh_id();
            state.ops.push(Op {
                id: op,
                label: format!("Process “{}”", local.name),
                txid: local.reveal_txid.clone(),
                layer: Layer::Bitcoin,
                stage: OpStage::Mempool,
            });
            fx.push(Effect::PollConfirmation { op, txid: local.reveal_txid.clone() });
            state.registry.definitions.retain(|d| d.id != local.id);
            state.registry.definitions.push(local.clone());
            fx.push(Effect::SaveRegistry);
            let registry = state.registry.clone();
            if let Some(tree) = state.tree_mut(&local.institution_id) {
                merge_pending_definitions(&registry, tree);
            }
            state.toast(ToastKind::Success, format!("Process “{}” broadcast. It appears as pending until it confirms.", local.name));
        }
        Msg::DefineFailed(why) => {
            if let Some(draft) = state.define.as_mut() {
                draft.busy = false;
                draft.error = Some(why);
            }
        }

        // ── Instances on Citrea ──────────────────────────────────
        Msg::StartInstance { institution, definition } => {
            if state.evm.is_none() {
                state.picker = Some(Layer::Citrea);
            } else if state.busy.is_none() {
                let steps = state.tree(&institution).and_then(|t| t.definition(&definition)).map(|d| d.steps.clone());
                if let Some(steps) = steps {
                    state.busy = Some(Busy::StartingInstance(definition.clone()));
                    fx.push(Effect::StartInstance { institution, definition, steps });
                }
            }
        }
        Msg::InstanceStarted { institution, definition, result } => {
            state.busy = None;
            match result {
                Ok((address, _tx)) => {
                    state.toast(ToastKind::Success, "Instance created on Citrea testnet.");
                    fx.push(Effect::LoadInstances { institution: institution.clone(), definition });
                    state.selected = Some(NodeRef::Instance { institution, address });
                }
                Err(why) => state.toast(ToastKind::Error, format!("Could not start the instance: {why}")),
            }
        }
        Msg::ExecuteStep { institution, definition, address } => {
            if state.evm.is_none() {
                state.picker = Some(Layer::Citrea);
            } else if state.busy.is_none() {
                let step = state.tree(&institution).and_then(|t| t.instance(&address)).map(|(_, i)| i.current_step);
                if let Some(step) = step {
                    state.busy = Some(Busy::ExecutingStep(address.clone()));
                    fx.push(Effect::ExecuteStep { institution, definition, address, step });
                }
            }
        }
        Msg::StepExecuted { institution, definition, address: _, result } => {
            state.busy = None;
            match result {
                Ok(_tx) => {
                    state.toast(ToastKind::Success, "Step executed on Citrea testnet.");
                    fx.push(Effect::LoadInstances { institution, definition });
                }
                Err(why) => state.toast(ToastKind::Error, format!("Could not execute the step: {why}")),
            }
        }

        Msg::OpUpdate { id, stage } => {
            if let Some(op) = state.ops.iter_mut().find(|o| o.id == id) {
                op.stage = stage;
            }
        }

        // ── Members and roles (Nostr) ────────────────────────────
        Msg::ReloadMembers(id) => {
            state.members.insert(id.clone(), Load::Loading);
            fx.push(Effect::LoadMembers(id));
        }
        Msg::MembersLoaded(id, result) => {
            state.members.insert(id, match result {
                Ok(view) => Load::Ready(view),
                Err(error) => Load::Failed(error),
            });
        }
        Msg::BindNostrKey(institution) => {
            state.members_error = None;
            let tree = state.tree(&institution).cloned();
            match tree {
                _ if state.btc.is_none() => state.picker = Some(Layer::Bitcoin),
                Some(tree) if state.holds(&tree) && state.members_busy.is_none() => {
                    state.members_busy = Some("Asking your Nostr signer for its public key…".to_string());
                    fx.push(Effect::BindNostrKey {
                        institution,
                        name: tree.institution.name.clone(),
                        holder: tree.institution.holder.clone(),
                    });
                }
                Some(_) if state.members_busy.is_none() => {
                    state.members_error = Some("Only the wallet that holds the institution can bind a Nostr key.".to_string());
                }
                _ => {}
            }
        }
        Msg::DefineRole { institution, name, description } => {
            let name = name.trim().to_string();
            let slug = crate::nostr::slug(&name);
            state.members_error = None;
            match state.bound_issuer(&institution) {
                _ if state.members_busy.is_some() => {}
                None => state.members_error = Some("Bind a Nostr key to the institution first.".to_string()),
                Some(_) if slug.is_empty() => state.members_error = Some("Give the role a name.".to_string()),
                Some(issuer) => {
                    state.members_busy = Some(format!("Publishing the role “{name}”…"));
                    fx.push(Effect::PublishRole { institution, issuer, slug, name, description: description.trim().to_string() });
                }
            }
        }
        Msg::AwardRole { institution, slug, member } => {
            state.members_error = None;
            match (state.bound_issuer(&institution), crate::nostr::parse_pubkey(&member)) {
                _ if state.members_busy.is_some() => {}
                (None, _) => state.members_error = Some("Bind a Nostr key to the institution first.".to_string()),
                (_, Err(why)) => state.members_error = Some(format!("Member key: {why}.")),
                (Some(issuer), Ok(member)) => {
                    state.members_busy = Some("Publishing the award…".to_string());
                    fx.push(Effect::PublishAward { institution, issuer, slug, member });
                }
            }
        }
        Msg::SetRevoked { institution, slug, member, revoked } => {
            state.members_error = None;
            let current = match state.members.get(&institution) {
                Some(Load::Ready(view)) => view.membership.as_ref().map(|m| (m.binding.issuer.clone(), m.revoked())),
                _ => None,
            };
            if let (Some((issuer, mut list)), None) = (current, state.members_busy.as_ref()) {
                list.retain(|(m, s)| !(*m == member && *s == slug));
                if revoked {
                    list.push((member, slug));
                }
                state.members_busy = Some("Publishing the revocation list…".to_string());
                fx.push(Effect::PublishRevocations { institution, issuer, revoked: list });
            }
        }
        Msg::MembersProgress(text) => state.members_busy = Some(text),
        Msg::MembersDone { institution, text } => {
            state.members_busy = None;
            state.members_error = None;
            state.toast(ToastKind::Success, text);
            state.members.insert(institution.clone(), Load::Loading);
            fx.push(Effect::LoadMembers(institution));
        }
        Msg::MembersFailed(why) => {
            state.members_busy = None;
            state.members_error = Some(why);
        }
        Msg::ReloadAnnounced => {
            state.announced = Load::Loading;
            fx.push(Effect::LoadAnnounced);
        }
        Msg::AnnouncedLoaded(result) => {
            state.announced = match result {
                Ok(list) => Load::Ready(list),
                Err(error) => Load::Failed(error),
            };
        }
    }
    fx.extend(state.new_toasts.drain(..).map(Effect::ExpireToast));
    fx
}

/// Add definitions this browser broadcast that the indexer has not listed yet.
fn merge_pending_definitions(registry: &Registry, tree: &mut Tree) {
    for local in registry.definitions.iter().filter(|d| d.institution_id == tree.institution.id) {
        if tree.definitions.iter().any(|d| d.id == local.id) {
            continue;
        }
        tree.definitions.push(Definition {
            id: local.id.clone(),
            name: local.name.clone(),
            description: local.description.clone(),
            steps: local.steps.clone(),
            provenance: Provenance::Pending,
            instances: Vec::new(),
            instances_stage: Stage::Ready,
        });
    }
}

/// A tree built only from this browser's registry, for an institution the
/// indexer does not know yet.
pub fn local_tree(registry: &Registry, id: &str) -> Option<Tree> {
    let local = registry.institutions.iter().find(|i| i.id == id)?;
    let mut tree = Tree {
        institution: crate::model::Institution {
            id: local.id.clone(),
            name: local.name.clone(),
            description: local.description.clone(),
            website: local.website.clone(),
            admin: local.admin.clone(),
            holder: local.holder.clone(),
            pending: true, height: 0,
        },
        definitions: Vec::new(),
        definitions_stage: Stage::Ready,
        source: Source::Local,
        notes: vec!["Waiting for the indexer to see this inscription. It usually takes one block.".to_string()],
    };
    merge_pending_definitions(registry, &mut tree);
    Some(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Institution;

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
    const DEF: &str = "505eb39ed86053d5568a23bd128cd153142a21de02326e15354cb70059901e40i0";

    fn tree(holder: &str) -> Tree {
        Tree {
            institution: Institution {
                id: ID.into(), name: "Acme".into(), description: String::new(), website: String::new(),
                admin: String::new(), holder: holder.into(), pending: false, height: 0,
            },
            definitions: Vec::new(),
            definitions_stage: Stage::Loading,
            source: Source::Live,
            notes: Vec::new(),
        }
    }

    fn definition() -> Definition {
        Definition {
            id: DEF.into(), name: "KYC".into(), description: String::new(),
            steps: vec![Step { name: "Review".into(), action: "approval".into() }],
            provenance: Provenance::Verified, instances: Vec::new(), instances_stage: Stage::Loading,
        }
    }

    fn btc(address: &str) -> BtcSession {
        BtcSession { address: address.into(), wallet_name: "UniSat".into(), ..BtcSession::default() }
    }

    #[test]
    fn opening_an_institution_loads_its_tree_once() {
        let mut s = AppState::new(ID);
        let route = Route::Institution { id: ID.into(), definition: None };
        assert_eq!(update(&mut s, Msg::Navigate(route.clone())), vec![Effect::LoadTree(ID.into())]);
        assert_eq!(s.trees.get(ID), Some(&Load::Loading));
        assert!(update(&mut s, Msg::Navigate(route)).is_empty());
    }

    #[test]
    fn a_definition_route_selects_that_definition() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::Navigate(Route::Institution { id: ID.into(), definition: Some(DEF.into()) }));
        assert_eq!(s.selected, Some(NodeRef::Definition { institution: ID.into(), id: DEF.into() }));
    }

    #[test]
    fn tree_stages_fill_in_and_request_the_next() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::Navigate(Route::Institution { id: ID.into(), definition: None }));
        let fx = update(&mut s, Msg::TreeLoaded(ID.into(), Ok(tree("tb1holder"))));
        assert_eq!(fx, vec![Effect::LoadDefinitions(ID.into())]);
        let fx = update(&mut s, Msg::DefinitionsLoaded(ID.into(), Ok(vec![definition()])));
        assert_eq!(fx, vec![Effect::LoadInstances { institution: ID.into(), definition: DEF.into() }]);
        let instance = Instance { address: "0xabc".into(), creator: String::new(), current_step: 0, total_steps: 1, completed: false };
        update(&mut s, Msg::InstancesLoaded { institution: ID.into(), definition: DEF.into(), result: Ok(vec![instance]) });
        let t = s.tree(ID).unwrap();
        assert_eq!(t.definitions[0].instances.len(), 1);
        assert_eq!(t.definitions[0].instances_stage, Stage::Ready);
    }

    #[test]
    fn a_failed_load_is_kept_as_a_visible_error() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::TreeLoaded(ID.into(), Err(UserError::new("Loading the institution", "indexer HTTP 429"))));
        assert!(matches!(s.trees.get(ID), Some(Load::Failed(e)) if e.why.contains("429")));
        update(&mut s, Msg::TreeLoaded(ID.into(), Ok(tree(""))));
        update(&mut s, Msg::DefinitionsLoaded(ID.into(), Err("indexer HTTP 502".into())));
        assert_eq!(s.tree(ID).unwrap().definitions_stage, Stage::Failed("indexer HTTP 502".into()));
    }

    #[test]
    fn connecting_bitcoin_saves_the_session_and_reloads_mine() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::OpenPicker(Layer::Bitcoin));
        update(&mut s, Msg::SetPaymentAddress("tb1qpay".into()));
        let fx = update(&mut s, Msg::BtcConnected(btc("tb1pord")));
        assert!(fx.contains(&Effect::SaveSessions));
        assert!(fx.contains(&Effect::LoadMine));
        assert_eq!(s.picker, None);
        assert_eq!(s.btc.as_ref().unwrap().payment_address, "tb1qpay");
        assert!(s.pending_payment_address.is_empty());
    }

    #[test]
    fn actions_without_a_wallet_open_the_right_picker() {
        let mut s = AppState::new(ID);
        assert!(update(&mut s, Msg::DefineOpen(ID.into())).is_empty());
        assert_eq!(s.picker, Some(Layer::Bitcoin));
        assert!(s.define.is_none());
        update(&mut s, Msg::StartInstance { institution: ID.into(), definition: DEF.into() });
        assert_eq!(s.picker, Some(Layer::Citrea));
    }

    #[test]
    fn holder_check_compares_the_connected_address() {
        let mut s = AppState::new(ID);
        assert!(!s.holds(&tree("tb1holder")));
        s.btc = Some(btc("tb1holder"));
        assert!(s.holds(&tree("tb1holder")));
        assert!(!s.holds(&tree("tb1other")));
        assert!(!s.holds(&tree("")));
    }

    #[test]
    fn create_validates_before_review() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::CreateReview { name: "  ".into(), description: String::new(), website: String::new() });
        assert_eq!(s.create.step, CreateStep::Details);
        assert!(s.create.error.is_some());
        update(&mut s, Msg::CreateReview { name: "Acme".into(), description: String::new(), website: "http://x".into() });
        assert!(s.create.error.as_deref().unwrap().contains("https://"));
        let fx = update(&mut s, Msg::CreateReview { name: " Acme ".into(), description: "d".into(), website: String::new() });
        assert_eq!(s.create.step, CreateStep::Review);
        assert_eq!(s.create.name, "Acme");
        assert_eq!(fx, vec![Effect::FetchFeeRate]);
    }

    #[test]
    fn create_submit_needs_a_bitcoin_wallet_then_inscribes() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::CreateReview { name: "Acme".into(), description: String::new(), website: String::new() });
        update(&mut s, Msg::FeeRate(3));
        assert!(update(&mut s, Msg::CreateSubmit).is_empty());
        assert_eq!(s.picker, Some(Layer::Bitcoin));
        s.btc = Some(btc("tb1p"));
        let fx = update(&mut s, Msg::CreateSubmit);
        assert_eq!(s.create.step, CreateStep::Signing);
        assert!(matches!(&fx[0], Effect::InscribeInstitution { name, fee_rate: 3, .. } if name == "Acme"));
    }

    #[test]
    fn a_created_institution_is_registered_tracked_and_listed() {
        let mut s = AppState::new(ID);
        s.mine = Load::Ready(vec![]);
        let local = LocalInstitution {
            id: DEF.into(), reveal_txid: "aa".into(), name: "New".into(), description: String::new(),
            website: String::new(), admin: String::new(), holder: "tb1p".into(),
        };
        let fx = update(&mut s, Msg::CreateDone(local));
        assert!(fx.contains(&Effect::SaveRegistry));
        assert!(fx.iter().any(|e| matches!(e, Effect::PollConfirmation { txid, .. } if txid == "aa")));
        assert_eq!(s.mine, Load::Ready(vec![DEF.to_string()]));
        assert_eq!(s.ops.len(), 1);
        let t = local_tree(&s.registry, DEF).unwrap();
        assert!(t.institution.pending);
        assert_eq!(t.source, Source::Local);
    }

    #[test]
    fn define_adds_removes_and_validates_steps() {
        let mut s = AppState::new(ID);
        s.btc = Some(btc("tb1p"));
        update(&mut s, Msg::DefineOpen(ID.into()));
        let one = vec![Step { name: "Review".into(), action: "approval".into() }];
        update(&mut s, Msg::DefineEdit { name: "KYC".into(), description: String::new(), steps: one.clone(), then: DefineThen::AddStep });
        assert_eq!(s.define.as_ref().unwrap().steps.len(), 2);
        let two = s.define.as_ref().unwrap().steps.clone();
        let fx = update(&mut s, Msg::DefineEdit { name: "KYC".into(), description: String::new(), steps: two.clone(), then: DefineThen::Submit });
        assert!(fx.is_empty());
        assert!(s.define.as_ref().unwrap().error.as_deref().unwrap().contains("step"));
        update(&mut s, Msg::DefineEdit { name: "KYC".into(), description: String::new(), steps: two, then: DefineThen::RemoveStep(1) });
        let fx = update(&mut s, Msg::DefineEdit { name: "KYC".into(), description: String::new(), steps: one, then: DefineThen::Submit });
        assert!(matches!(&fx[0], Effect::InscribeDefinition { institution, .. } if institution == ID));
        assert!(s.define.as_ref().unwrap().busy);
    }

    #[test]
    fn a_broadcast_definition_shows_as_pending_on_its_tree() {
        let mut s = AppState::new(ID);
        update(&mut s, Msg::TreeLoaded(ID.into(), Ok(Tree { definitions_stage: Stage::Ready, ..tree("tb1p") })));
        let local = LocalDefinition {
            id: DEF.into(), reveal_txid: "bb".into(), institution_id: ID.into(), name: "KYC".into(),
            description: String::new(), steps: vec![],
        };
        update(&mut s, Msg::DefineDone(local));
        let t = s.tree(ID).unwrap();
        assert_eq!(t.definitions.len(), 1);
        assert_eq!(t.definitions[0].provenance, Provenance::Pending);
        // The indexer catching up replaces the pending copy instead of duplicating it.
        update(&mut s, Msg::DefinitionsLoaded(ID.into(), Ok(vec![definition()])));
        let t = s.tree(ID).unwrap();
        assert_eq!(t.definitions.len(), 1);
        assert_eq!(t.definitions[0].provenance, Provenance::Verified);
    }

    #[test]
    fn executing_a_step_uses_the_current_step_and_blocks_double_clicks() {
        let mut s = AppState::new(ID);
        s.evm = Some(EvmSession { address: "0x1".into(), network: "citrea-testnet".into(), ..EvmSession::default() });
        let mut t = tree("tb1p");
        let mut d = definition();
        d.instances = vec![Instance { address: "0xabc".into(), creator: String::new(), current_step: 1, total_steps: 3, completed: false }];
        t.definitions = vec![d];
        s.trees.insert(ID.into(), Load::Ready(t));
        let msg = Msg::ExecuteStep { institution: ID.into(), definition: DEF.into(), address: "0xabc".into() };
        let fx = update(&mut s, msg.clone());
        assert!(matches!(&fx[0], Effect::ExecuteStep { step: 1, .. }));
        assert!(update(&mut s, msg).is_empty());
    }

    fn members_view(bound: bool) -> MembersView {
        use crate::nostr::{Binding, Member, Role};
        MembersView {
            membership: bound.then(|| Membership {
                binding: Binding { issuer: "aa".repeat(32), holder: "tb1holder".into(), issued: 1 },
                roles: vec![Role {
                    slug: "board".into(), name: "Board".into(), description: String::new(),
                    members: vec![Member { pubkey: "bb".repeat(32), awarded_at: 2, revoked: true }],
                }],
            }),
            rejected: vec![],
            asked: 3,
            answered: 3,
        }
    }

    #[test]
    fn the_members_page_asks_the_relays_once_the_holder_is_known() {
        let mut s = AppState::new(ID);
        let fx = update(&mut s, Msg::Navigate(Route::Members { id: ID.into() }));
        assert_eq!(fx, vec![Effect::LoadTree(ID.into())]);
        let fx = update(&mut s, Msg::TreeLoaded(ID.into(), Ok(Tree { definitions_stage: Stage::Ready, ..tree("tb1holder") })));
        assert_eq!(fx, vec![Effect::LoadMembers(ID.into())]);
        assert_eq!(s.members.get(ID), Some(&Load::Loading));
        // Coming back to the page does not ask again.
        update(&mut s, Msg::MembersLoaded(ID.into(), Ok(members_view(false))));
        assert!(update(&mut s, Msg::Navigate(Route::Members { id: ID.into() })).is_empty());
    }

    #[test]
    fn only_the_holder_can_start_binding_a_nostr_key() {
        let mut s = AppState::new(ID);
        s.trees.insert(ID.into(), Load::Ready(tree("tb1holder")));
        assert!(update(&mut s, Msg::BindNostrKey(ID.into())).is_empty());
        assert_eq!(s.picker, Some(Layer::Bitcoin));
        s.btc = Some(btc("tb1other"));
        assert!(update(&mut s, Msg::BindNostrKey(ID.into())).is_empty());
        assert!(s.members_error.as_deref().unwrap().contains("holds the institution"));
        s.btc = Some(btc("tb1holder"));
        let fx = update(&mut s, Msg::BindNostrKey(ID.into()));
        assert!(matches!(&fx[0], Effect::BindNostrKey { holder, .. } if holder == "tb1holder"));
        assert!(s.members_busy.is_some());
        // A second click while the first is in flight does nothing.
        assert!(update(&mut s, Msg::BindNostrKey(ID.into())).is_empty());
    }

    #[test]
    fn roles_need_a_binding_a_name_and_a_valid_member_key() {
        let mut s = AppState::new(ID);
        let define = |name: &str| Msg::DefineRole { institution: ID.into(), name: name.into(), description: String::new() };
        assert!(update(&mut s, define("Board")).is_empty());
        assert!(s.members_error.as_deref().unwrap().contains("Bind a Nostr key"));
        s.members.insert(ID.into(), Load::Ready(members_view(true)));
        assert!(update(&mut s, define("  !! ")).is_empty());
        assert!(s.members_error.as_deref().unwrap().contains("name"));
        let award = |member: &str| Msg::AwardRole { institution: ID.into(), slug: "board".into(), member: member.into() };
        assert!(update(&mut s, award("npub1nope")).is_empty());
        assert!(s.members_error.as_deref().unwrap().starts_with("Member key"));
        let fx = update(&mut s, define("Board Member"));
        assert!(matches!(&fx[0], Effect::PublishRole { slug, issuer, .. } if slug == "board-member" && *issuer == "aa".repeat(32)));
        assert!(s.members_error.is_none());
    }

    #[test]
    fn revoking_and_reinstating_republish_the_whole_list() {
        let mut s = AppState::new(ID);
        s.members.insert(ID.into(), Load::Ready(members_view(true)));
        let member = "cc".repeat(32);
        let fx = update(&mut s, Msg::SetRevoked { institution: ID.into(), slug: "board".into(), member: member.clone(), revoked: true });
        let Effect::PublishRevocations { revoked, .. } = &fx[0] else { panic!("expected a revocation list") };
        assert_eq!(revoked, &vec![("bb".repeat(32), "board".to_string()), (member, "board".to_string())]);
        update(&mut s, Msg::MembersFailed("refused".into()));
        assert_eq!(s.members_error.as_deref(), Some("refused"));
        let fx = update(&mut s, Msg::SetRevoked { institution: ID.into(), slug: "board".into(), member: "bb".repeat(32), revoked: false });
        assert!(matches!(&fx[0], Effect::PublishRevocations { revoked, .. } if revoked.is_empty()));
    }

    #[test]
    fn the_nostr_lab_page_lists_announced_institutions_once() {
        let mut s = AppState::new(ID);
        let fx = update(&mut s, Msg::Navigate(Route::Lab(Some("nostr".into()))));
        assert_eq!(fx, vec![Effect::LoadAnnounced]);
        update(&mut s, Msg::AnnouncedLoaded(Ok(vec![(ID.to_string(), "Acme".to_string())])));
        assert!(update(&mut s, Msg::Navigate(Route::Lab(Some("nostr".into())))).is_empty());
        assert!(update(&mut s, Msg::Navigate(Route::Lab(Some("ark".into())))).is_empty());
    }

    #[test]
    fn a_finished_members_action_reloads_from_the_relays() {
        let mut s = AppState::new(ID);
        s.members_busy = Some("Publishing…".into());
        let fx = update(&mut s, Msg::MembersDone { institution: ID.into(), text: "Role published.".into() });
        assert!(fx.contains(&Effect::LoadMembers(ID.into())));
        assert!(s.members_busy.is_none());
        assert_eq!(s.members.get(ID), Some(&Load::Loading));
    }

    #[test]
    fn toasts_are_capped_and_expire() {
        let mut s = AppState::new(ID);
        for i in 0..6 {
            let fx = update(&mut s, Msg::Toast(ToastKind::Info, format!("t{i}")));
            assert!(fx.iter().any(|e| matches!(e, Effect::ExpireToast(_))));
        }
        assert_eq!(s.toasts.len(), 4);
        let first = s.toasts[0].id;
        update(&mut s, Msg::DismissToast(first));
        assert_eq!(s.toasts.len(), 3);
    }

    #[test]
    fn zoom_is_clamped_and_resets() {
        let mut s = AppState::new(ID);
        for _ in 0..20 {
            update(&mut s, Msg::Zoom(20));
        }
        assert_eq!(s.zoom, 160);
        update(&mut s, Msg::Zoom(0));
        assert_eq!(s.zoom, 100);
    }
}
