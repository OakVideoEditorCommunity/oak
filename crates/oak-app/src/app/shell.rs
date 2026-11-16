use super::*;

pub(crate) mod modal_ids {
	pub const PREFERENCES: usize = 3;
	pub const EXPORT: usize = 4;
	pub const EXPORT_PROGRESS: usize = 5;
	pub const MANAGER: usize = 6;
	pub const MANAGER_RENAME: usize = 7;
	pub const MANAGER_DELETE: usize = 8;
	pub const PROXY: usize = 9;
	/// The OFX plugin progress dialog (driven by the plugin-progress
	/// channel in the tick loop).
	pub const PLUGIN_PROGRESS: usize = 10;
	/// The action search dialog (Help > Search Actions…, the `/` key).
	pub const ACTION_SEARCH: usize = 11;
	/// The about dialog (Help > About Oak…).
	pub const ABOUT: usize = 13;
	/// The project properties dialog (File > Project Properties…).
	pub const PROJECT_PROPERTIES: usize = 12;
	/// The new-sequence dialog (File > New > Sequence…).
	pub const NEW_SEQUENCE: usize = 14;
	/// The sequence properties dialog (right-click a sequence in the
	/// project explorer > Sequence Properties).
	pub const SEQUENCE_PROPERTIES: usize = 15;
	/// The drop-onto-empty-timeline choice (probe the footage's params as
	/// the sequence's, or set them up manually).
	pub const DROP_SEQUENCE_CHOICE: usize = 16;
	/// The multicam wizard (窗口 → 多机位向导).
	pub const MULTICAM_WIZARD: usize = 17;
	/// The rename dialog (project explorer 重命名).
	pub const RENAME: usize = 18;
	/// The project export dialog (文件 → 导出工程文件…).
	pub const EXPORT_PROJECT_DFD: usize = 19;
	/// The new-project dialog (文件 → 新建项目).
	pub const NEW_PROJECT: usize = 20;
	/// The save-as dialog (文件 → 另存为).
	pub const SAVE_AS: usize = 21;
	/// The update-available prompt (the startup update check).
	pub const UPDATE_AVAILABLE: usize = 22;
	/// The clip rename prompt (编辑 → 重命名 on the timeline).
	pub const CLIP_RENAME: usize = 23;
	/// The speed/duration prompt (编辑 → 速度/持续时间 on the timeline).
	pub const CLIP_SPEED: usize = 24;
	/// Subtitle font/size/outline editor.
	pub const SUBTITLE_PROPERTIES: usize = 25;
	/// Per-clip audio gain/pan/fade + effect parameter editor.
	pub const AUDIO_PROPERTIES: usize = 26;
}

/// What a picked platform-dialog path should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileAction {
	ImportFootage,
	Open,
	/// Export the current project to a file (the 导出工程文件… action's
	/// target; `.ove` / `.otio` / `.fcpxml`, dispatched by extension).
	// Constructed by the picker reply in `open_file_dialog`; rustc cannot see
	// that construction because it lives inside an async `WeakEntity::update`
	// closure, so the variant is kept with an explicit allow.
	#[allow(dead_code)]
	ExportProjectFile,
	/// Import an SRT file as text clips on the current sequence.
	ImportSubtitles,
	/// Export the current sequence's text clips as SRT.
	ExportSubtitles,
	/// Import a project file into the library (the manager's 导入).
	ImportProject,
	/// Export the selected library project (the manager's 导出; the row's
	/// uuid is stashed in [`OakApp::pending_export`]).
	ExportProject,
}

/// One coarse audio-post operation from the Sequence > Audio menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AudioEdit {
	Gain(f64),
	Pan(f64),
	FadeIn,
	FadeOut,
	AddVolume,
	AddPan,
}

/// The modal currently layered on top of the shell, if any.
pub(crate) enum ModalState<E: AppEngine> {
	None,
	Preferences {
		modal: Entity<Modal>,
		content: Entity<PreferencesDialogContent>,
	},
	Export {
		modal: Entity<Modal>,
		content: Entity<ExportDialogContent>,
	},
	Progress {
		modal: Entity<Modal>,
		content: Entity<ProgressContent>,
	},
	/// The project manager (M13 D4).
	Manager {
		modal: Entity<Modal>,
		content: Entity<crate::manager::ProjectManager<E>>,
	},
	/// The manager's rename prompt.
	ManagerRename {
		modal: Entity<Modal>,
		content: Entity<crate::manager::NamePrompt>,
		uuid: String,
	},
	/// The timeline's clip rename prompt (编辑 → 重命名).
	ClipRename {
		modal: Entity<Modal>,
		content: Entity<crate::manager::NamePrompt>,
		clips: Vec<ClipId>,
	},
	/// The timeline's speed/duration prompt (编辑 → 速度/持续时间).
	ClipSpeed {
		modal: Entity<Modal>,
		content: Entity<crate::manager::NamePrompt>,
		clips: Vec<ClipId>,
	},
	/// The manager's delete confirmation.
	ManagerDelete {
		modal: Entity<Modal>,
		uuid: String,
	},
	/// The proxy settings dialog (Tools > Proxy Settings).
	Proxy {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::ProxyDialogContent<E>>,
	},
	/// The action search dialog (Help > Search Actions…, the `/` key).
	ActionSearch {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::ActionSearchContent>,
	},
	/// The project properties dialog (File > Project Properties…; the
	/// per-project OCIO config override and the disk-cache location).
	ProjectProperties {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::ProjectPropertiesContent<E>>,
	},
	/// The about dialog (Help > About Oak…; static content).
	About {
		modal: Entity<Modal>,
	},
	/// The update-available prompt (the startup check; the dialog itself
	/// carries the version and notes).
	UpdateAvailable {
		modal: Entity<Modal>,
	},
	/// The new-sequence dialog (File > New > Sequence…).
	NewSequence {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::NewSequenceContent<E>>,
	},
	/// The sequence properties dialog (project-explorer context menu >
	/// Sequence Properties).
	SequenceProperties {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::SequencePropertiesContent<E>>,
	},
	/// The drop-onto-empty-timeline choice (拖入素材到没有序列的时间轴):
	/// probe the footage's params as the sequence's, or open the manual
	/// setup dialog. The skipped drop is resumed by the shell once a
	/// sequence exists (the paused drop lives in [`OakApp::pending_drop`]).
	/// The buttons carry the decision, so the state keeps only the modal.
	DropSequenceChoice {
		modal: Entity<Modal>,
	},
	/// The multicam wizard (窗口 → 多机位向导): pick the angle footage,
	/// choose the sync mode, then create the multicam sequence. The OK
	/// button runs the sync + creation through the content.
	MulticamWizard {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::MulticamWizardContent<E>>,
	},
	/// Subtitle font/size/outline editor.
	SubtitleProperties {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::SubtitleStyleContent>,
	},
	/// Per-clip audio gain/pan/fade + effect parameter editor.
	AudioProperties {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::AudioPropertiesContent>,
	},
	/// The project-explorer rename dialog (right-click → 重命名): the
	/// content's field is prefilled with the entry's current name.
	EntryRename {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::RenameContent>,
		entry_id: u64,
	},
	/// The project export dialog (文件 → 导出工程文件…): choose the
	/// project format (OTIO / OVE / FCPXML) + the output path.
	ExportProjectDfd {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::ExportProjectDialogContent>,
	},
	/// The new-project dialog (文件 → 新建项目): name + sequence format.
	NewProject {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::NewProjectContent>,
	},
	/// The save-as dialog (文件 → 另存为): the copy's name.
	SaveAs {
		modal: Entity<Modal>,
		content: Entity<crate::dialogs::RenameContent>,
	},
}

/// A running export: the session the tick loop drains for progress.
impl<E: AppEngine> ModalState<E> {
	/// The modal entity currently shown, if any.
	pub(crate) fn modal_entity(&self) -> Option<Entity<Modal>> {
		match self {
			ModalState::None => None,
			ModalState::Preferences { modal, .. }
			| ModalState::Export { modal, .. }
			| ModalState::Progress { modal, .. }
			| ModalState::Manager { modal, .. }
			| ModalState::ManagerRename { modal, .. }
			| ModalState::ClipRename { modal, .. }
			| ModalState::ClipSpeed { modal, .. }
			| ModalState::ManagerDelete { modal, .. }
			| ModalState::Proxy { modal, .. }
			| ModalState::ActionSearch { modal, .. }
			| ModalState::ProjectProperties { modal, .. }
			| ModalState::About { modal }
			| ModalState::UpdateAvailable { modal }
			| ModalState::NewSequence { modal, .. }
			| ModalState::SequenceProperties { modal, .. }
			| ModalState::DropSequenceChoice { modal, .. }
			| ModalState::MulticamWizard { modal, .. }
			| ModalState::SubtitleProperties { modal, .. }
			| ModalState::AudioProperties { modal, .. }
			| ModalState::EntryRename { modal, .. } => Some(modal.clone()),
			ModalState::ExportProjectDfd { modal, .. } => Some(modal.clone()),
			ModalState::NewProject { modal, .. } => Some(modal.clone()),
			ModalState::SaveAs { modal, .. } => Some(modal.clone()),
		}
	}
}

/// The application root view.
pub(crate) const CONFIG_KEY_DOCK_LAYOUT: &str = "Workspace/DockLayout";
pub(crate) const CONFIG_KEY_TIMELINE_VISIBLE: &str = "Workspace/TimelineVisible";
pub(crate) const CONFIG_KEY_TIMELINE_HEIGHT: &str = "Workspace/TimelineHeight";
pub(crate) const WORKSPACE_SAVE_DELAY: Duration = Duration::from_millis(500);

pub(crate) struct FloatingPanel {
	pub(crate) view: AnyView,
}

impl Render for FloatingPanel {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		self.view.clone()
	}
}

pub(crate) struct FloatingWindow {
	pub(crate) window_id: WindowId,
	pub(crate) _subscription: Subscription,
}

/// The dock panels the shell builds up front, kept for focused-panel command
/// routing (the dock area only exposes type-erased handles).
/// The shell's panel entities, kept for focused-panel command routing and
/// the 窗口 menu's open/closed toggles (the dock hands back type-erased
/// handles).
pub(crate) struct ShellPanels<E: AppEngine> {
	pub(crate) project: Entity<ProjectExplorerPanel<E>>,
	pub(crate) source_viewer: Entity<SourceViewerPanel<E>>,
	pub(crate) program_viewer: Entity<ProgramViewerPanel<E>>,
	pub(crate) node_editor: Entity<NodeEditorPanel<E>>,
	pub(crate) inspector: Entity<InspectorPanel<E>>,
	pub(crate) history: Entity<HistoryPanel<E>>,
	pub(crate) timeline: Entity<TimelinePanel<E>>,
	pub(crate) effect_library: Entity<EffectLibraryPanel<E>>,
	pub(crate) multicam: Entity<MulticamPanel<E>>,
	pub(crate) ai_assistant: Entity<AiAssistantPanel<E>>,
}

impl<E: AppEngine> Clone for ShellPanels<E> {
	fn clone(&self) -> Self {
		Self {
			project: self.project.clone(),
			source_viewer: self.source_viewer.clone(),
			program_viewer: self.program_viewer.clone(),
			node_editor: self.node_editor.clone(),
			inspector: self.inspector.clone(),
			history: self.history.clone(),
			timeline: self.timeline.clone(),
			effect_library: self.effect_library.clone(),
			multicam: self.multicam.clone(),
			ai_assistant: self.ai_assistant.clone(),
		}
	}
}

impl<E: AppEngine> ShellPanels<E> {
	pub(crate) fn register_for_layout_restore(&self, cx: &mut App) {
		macro_rules! register {
			($name:expr, $panel:expr) => {{
				let panel = $panel.clone();
				register_panel(cx, $name, move |_, _, _| panel_handle(panel.clone()));
			}};
		}
		register!(PROJECT, self.project);
		register!(SOURCE_VIEWER, self.source_viewer);
		register!(PROGRAM_VIEWER, self.program_viewer);
		register!(NODE_EDITOR, self.node_editor);
		register!(INSPECTOR, self.inspector);
		register!(HISTORY, self.history);
		register!(TIMELINE, self.timeline);
		register!(EFFECT_LIBRARY, self.effect_library);
		register!(MULTICAM, self.multicam);
	}

	pub(crate) fn id_of_raw(&self, raw: u64) -> Option<PanelId> {
		let panels = [
			(PROJECT, self.project.entity_id().as_u64()),
			(SOURCE_VIEWER, self.source_viewer.entity_id().as_u64()),
			(PROGRAM_VIEWER, self.program_viewer.entity_id().as_u64()),
			(NODE_EDITOR, self.node_editor.entity_id().as_u64()),
			(INSPECTOR, self.inspector.entity_id().as_u64()),
			(HISTORY, self.history.entity_id().as_u64()),
			(TIMELINE, self.timeline.entity_id().as_u64()),
			(EFFECT_LIBRARY, self.effect_library.entity_id().as_u64()),
			(MULTICAM, self.multicam.entity_id().as_u64()),
		];
		panels
			.into_iter()
			.find_map(|(id, entity)| (entity == raw).then_some(id))
	}

	pub(crate) fn floating_view(&self, id: PanelId) -> Option<AnyView> {
		Some(match id {
			PROJECT => panel_handle(self.project.clone()).view(),
			SOURCE_VIEWER => panel_handle(self.source_viewer.clone()).view(),
			PROGRAM_VIEWER => panel_handle(self.program_viewer.clone()).view(),
			NODE_EDITOR => panel_handle(self.node_editor.clone()).view(),
			INSPECTOR => panel_handle(self.inspector.clone()).view(),
			HISTORY => panel_handle(self.history.clone()).view(),
			EFFECT_LIBRARY => panel_handle(self.effect_library.clone()).view(),
			MULTICAM => panel_handle(self.multicam.clone()).view(),
			_ => return None,
		})
	}

	/// The entity id of a panel, for dock presence checks
	/// ([`DockArea::panel`] keys by it).
	pub(crate) fn entity_id(&self, id: PanelId) -> Option<gpui_kit::EntityId> {
		Some(match id {
			PROJECT => self.project.entity_id(),
			SOURCE_VIEWER => self.source_viewer.entity_id(),
			PROGRAM_VIEWER => self.program_viewer.entity_id(),
			NODE_EDITOR => self.node_editor.entity_id(),
			INSPECTOR => self.inspector.entity_id(),
			HISTORY => self.history.entity_id(),
			TIMELINE => self.timeline.entity_id(),
			EFFECT_LIBRARY => self.effect_library.entity_id(),
			MULTICAM => self.multicam.entity_id(),
			AI_ASSISTANT => self.ai_assistant.entity_id(),
			_ => return None,
		})
	}
}
