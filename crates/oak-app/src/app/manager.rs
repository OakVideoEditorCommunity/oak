use super::*;

impl<E: AppEngine> OakApp<E> {
	/// Opens the project manager (startup without a project argument, and
	/// 文件 → 项目管理器 / 从库中打开…). Re-entrant: an already-open
	/// manager just reloads its list.
	pub fn show_project_manager(&mut self, cx: &mut Context<Self>) {
		if let ModalState::Manager { content, .. } = &self.modal {
			let content = content.clone();
			content.update(cx, |manager, cx| manager.reload(cx));
			return;
		}
		let engine = self.engine.clone();
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::manager::ProjectManager::new(engine, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::MANAGER,
					ModalOptions::new(crate::i18n::tr("manager.title"), px(880.0))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.close"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::Manager { modal, content }
		});
		// The content's requests (open / create / rename / ...) route here.
		if let ModalState::Manager { content, .. } = &self.modal {
			let content = content.clone();
			cx.subscribe(
				&content,
				|this, _content, event: &crate::manager::ManagerEvent, cx| {
					this.on_manager_event(event, cx);
				},
			)
			.detach();
		}
	}

	/// Dispatch one request from the project manager dialog to the engine.
	///
	/// `event` is the manager's [`crate::manager::ManagerEvent`]; `cx` is the
	/// entity context. Each variant maps to one library operation:
	/// - `Create` creates a project named with the localized default
	///   `manager.new.default_name` and closes the manager on success.
	/// - `Open(uuid)` opens that project and closes the manager on success.
	/// - `Rename(uuid)` / `Delete(uuid)` swap the dialog for the respective
	///   prompt ([`Self::open_manager_rename`],
	///   [`Self::open_manager_delete`]); the engine call happens on confirm.
	/// - `Duplicate(uuid)` duplicates the row and reloads the list.
	/// - `Import` / `Export(uuid)` open the platform file dialog
	///   ([`Self::open_file_dialog`] / [`Self::open_manager_export`]).
	///
	/// Failures of Create/Open/Duplicate go to the manager's status line via
	/// [`Self::manager_status`] and leave the dialog open, except that a
	/// successful Open/Create closes it. This is installed as a subscription
	/// on the manager content by [`Self::show_project_manager`], so it runs
	/// for every manager request while that dialog lives.
	pub(crate) fn on_manager_event(
		&mut self,
		event: &crate::manager::ManagerEvent,
		cx: &mut Context<Self>,
	) {
		use crate::manager::ManagerEvent as E;
		match event {
			E::Create => {
				let name = crate::i18n::tr("manager.new.default_name").to_string();
				let result = self
					.engine
					.update(cx, |engine, cx| engine.library_create_project(&name, cx));
				match result {
					Ok(()) => self.close_modal(cx),
					Err(err) => self.manager_status(err, cx),
				}
			}
			E::Open(uuid) => {
				let uuid = uuid.clone();
				let result = self
					.engine
					.update(cx, |engine, cx| engine.library_open_project(&uuid, cx));
				match result {
					Ok(()) => self.close_modal(cx),
					Err(err) => self.manager_status(err, cx),
				}
			}
			E::Rename(uuid) => self.open_manager_rename(uuid.clone(), cx),
			E::Duplicate(uuid) => {
				let result = self
					.engine
					.update(cx, |engine, _cx| engine.library_duplicate_project(uuid));
				match result {
					Ok(()) => self.reload_manager(cx),
					Err(err) => self.manager_status(err, cx),
				}
			}
			E::Delete(uuid) => self.open_manager_delete(uuid.clone(), cx),
			E::Import => self.open_file_dialog(FileAction::ImportProject, cx),
			E::Export(uuid) => self.open_manager_export(uuid.clone(), cx),
		}
	}

	/// Refresh the open project manager's row list after a library mutation.
	///
	/// `cx` is the entity context. Does nothing when the current modal is not
	/// the manager (e.g. after it was closed); otherwise calls
	/// `ProjectManager::reload` once, so the visible rows and their store
	/// entries are re-read. Used by the create/duplicate/import paths on
	/// success; rename and delete go back through
	/// [`Self::back_to_manager`], which re-creates the manager instead.
	pub(crate) fn reload_manager(&mut self, cx: &mut Context<Self>) {
		if let ModalState::Manager { content, .. } = &self.modal {
			let content = content.clone();
			content.update(cx, |manager, cx| manager.reload(cx));
		}
	}

	/// Show `message` in the open project manager's status line.
	///
	/// `message` is the raw engine error text (already user-facing, not
	/// translated here); `cx` is the entity context. When the manager modal is
	/// the current one the message is written through
	/// [`crate::manager::ProjectManager::set_status`] and replaces any previous
	/// status line; when the manager is gone (closed, or another modal is up)
	/// the message only goes to stdout as `[manager] …`, so an error raised
	/// after the dialog closed is not lost from the log. Used by the
	/// library-mutating paths (rename/delete/duplicate/import/export).
	pub(crate) fn manager_status(&mut self, message: String, cx: &mut Context<Self>) {
		if let ModalState::Manager { content, .. } = &self.modal {
			let content = content.clone();
			content.update(cx, |manager, cx| manager.set_status(Some(message), cx));
		} else {
			println!("[manager] {message}");
		}
	}

	/// The selected row's display name in the open manager (used to seed
	/// the rename prompt / the delete confirmation / the export filename).
	pub(crate) fn manager_selected_name(&self, cx: &App) -> Option<String> {
		match &self.modal {
			ModalState::Manager { content, .. } => content.read(cx).selected_name(),
			_ => None,
		}
	}

	/// Swap the manager for a rename prompt for the library row `uuid`.
	///
	/// `uuid` is the row to rename; `cx` is the entity context. The prompt is
	/// pre-filled with the manager's currently selected row name
	/// ([`Self::manager_selected_name`], empty when nothing is selected — the
	/// name is only a hint, the uuid is what gets renamed). Buttons: primary
	/// (labelled `manager.rename.title`) and Cancel. The primary route is
	/// [`Self::confirm_manager_rename`]; Escape/Cancel returns to the manager.
	/// State: [`ModalState::ManagerRename`], which keeps the uuid next to the
	/// prompt entity.
	pub(crate) fn open_manager_rename(&mut self, uuid: String, cx: &mut Context<Self>) {
		let initial = self.manager_selected_name(cx).unwrap_or_default();
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::manager::NamePrompt::new(&initial, window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::MANAGER_RENAME,
					ModalOptions::new(crate::i18n::tr("manager.rename.title"), px(380.0))
						.with_button(DialogButton::primary(crate::i18n::tr(
							"manager.rename.title",
						)))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.cancel"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ManagerRename {
				modal,
				content,
				uuid,
			}
		});
	}

	/// Open the 编辑 → 重命名 dialog for `clips` (timeline multi-clip rename).
	///
	/// `clips` are the timeline clip ids captured at invocation and stored in
	/// the modal, so the rename targets them even if the selection changes
	/// while the dialog is up; `cx` is the entity context. The prompt starts
	/// empty (the clips may currently have different names, so there is no
	/// single sensible prefill). On OK an empty name is ignored and the
	/// dialog just closes; otherwise `rename_clips(clips, name)` runs once
	/// for the whole batch (one engine update, so one undo step). Cancel
	/// closes without applying.
	///
	/// State: [`ModalState::ClipRename`], built via [`Self::spawn_modal`].
	pub(crate) fn open_clip_rename(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::manager::NamePrompt::new("", window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::CLIP_RENAME,
					ModalOptions::new(crate::i18n::tr("timeline.rename.title"), px(380.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.cancel"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ClipRename {
				modal,
				content,
				clips,
			}
		});
	}

	/// Open the 速度/持续时间 dialog for `clips` and prompt for a speed
	/// factor.
	///
	/// `clips` is the timeline selection captured by the caller (the dialog
	/// stores the ids, so it applies to the same clips even if the selection
	/// changes afterwards); `cx` is the entity context. The prompt starts
	/// with `1.0` (100% = normal speed). On OK the entered text is parsed as
	/// a plain factor (`2`, `2.0`) or a percentage (`200%` → 2.0) and applied
	/// only when it parses to a finite value `> 0`; anything else (empty,
	/// zero, negative, `NaN`, junk) is silently discarded, and the dialog
	/// still closes. Cancel/Escape closes without applying.
	///
	/// The resulting state is [`ModalState::ClipSpeed`]; the engine call is
	/// `set_clips_speed` through [`Self::on_modal`].
	pub(crate) fn open_clip_speed(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|cx| crate::manager::NamePrompt::new("1.0", window, cx));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::CLIP_SPEED,
					ModalOptions::new(crate::i18n::tr("timeline.speed.title"), px(380.0))
						.with_button(DialogButton::primary(crate::i18n::tr("dialog.ok")))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.cancel"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ClipSpeed {
				modal,
				content,
				clips,
			}
		});
	}

	/// Swap the manager for a delete confirmation of the library row `uuid`.
	///
	/// `uuid` identifies the project to delete; `cx` is the entity context.
	/// The confirmation text is `manager.delete.confirm` with `{name}`
	/// replaced by the manager's currently selected row name
	/// ([`Self::manager_selected_name`], empty if there is no selection, so
	/// the dialog still reads correctly). Buttons: primary "delete" and
	/// Cancel; the primary route is [`Self::confirm_manager_delete`], while
	/// Escape/Cancel returns to the manager (see [`Self::on_modal`]). The
	/// resulting modal state is [`ModalState::ManagerDelete`], which keeps
	/// only `uuid` (the prompt text lives in the content entity).
	pub(crate) fn open_manager_delete(&mut self, uuid: String, cx: &mut Context<Self>) {
		let name = self.manager_selected_name(cx).unwrap_or_default();
		let text = crate::i18n::tr("manager.delete.confirm").replace("{name}", &name);
		self.spawn_modal(cx, move |window, app| {
			let content = app.new(|_cx| crate::manager::ConfirmContent::new(text));
			let modal = app.new(|cx| {
				Modal::new(
					modal_ids::MANAGER_DELETE,
					ModalOptions::new(crate::i18n::tr("manager.delete.title"), px(420.0))
						.with_button(DialogButton::primary(crate::i18n::tr("manager.delete")))
						.with_button(DialogButton::cancel(crate::i18n::tr("dialog.cancel"))),
					window,
					cx,
				)
				.with_content(content.clone())
			});
			ModalState::ManagerDelete { modal, uuid }
		});
	}

	/// Confirm the rename prompt: apply the entered name to the project row,
	/// then return to the manager.
	///
	/// Requires `self.modal` to be [`ModalState::ManagerRename`] (otherwise
	/// no-op) and reads the row's `uuid` from it. An empty name (not trimmed
	/// here) cancels the rename and just goes back. Otherwise calls
	/// `library_rename_project(&uuid, &name)`; like
	/// [`Self::confirm_manager_delete`], success and failure both return to
	/// the manager and a failure is reported in the manager's status line
	/// ([`Self::manager_status`]). The list itself is refreshed because
	/// [`Self::back_to_manager`] re-creates the manager dialog (a fresh
	/// [`crate::manager::ProjectManager`] reloads its rows).
	///
	/// `cx` is the entity context.
	pub(crate) fn confirm_manager_rename(&mut self, cx: &mut Context<Self>) {
		let ModalState::ManagerRename { content, uuid, .. } = &self.modal else {
			return;
		};
		let name = content.read(cx).value(cx);
		let uuid = uuid.clone();
		if name.is_empty() {
			self.back_to_manager(cx);
			return;
		}
		let result = self.engine.update(cx, |engine, _cx| {
			engine.library_rename_project(&uuid, &name)
		});
		match result {
			Ok(()) => self.back_to_manager(cx),
			Err(err) => {
				self.back_to_manager(cx);
				self.manager_status(err, cx);
			}
		}
	}

	/// Confirm the delete confirmation dialog: delete the project from the
	/// library, then return to the manager.
	///
	/// Requires `self.modal` to be [`ModalState::ManagerDelete`] (otherwise
	/// no-op) and takes the row's `uuid` from it. Calls
	/// `library_delete_project(&uuid)`; success and failure both go back to
	/// the manager ([`Self::back_to_manager`]), a failure additionally
	/// surfaces the message in the manager's status line through
	/// [`Self::manager_status`]. There is no confirmation of the deletion
	/// beyond the dialog itself, and no undo.
	///
	/// `cx` is the entity context.
	pub(crate) fn confirm_manager_delete(&mut self, cx: &mut Context<Self>) {
		let ModalState::ManagerDelete { uuid, .. } = &self.modal else {
			return;
		};
		let uuid = uuid.clone();
		let result = self
			.engine
			.update(cx, |engine, _cx| engine.library_delete_project(&uuid));
		match result {
			Ok(()) => self.back_to_manager(cx),
			Err(err) => {
				self.back_to_manager(cx);
				self.manager_status(err, cx);
			}
		}
	}

	/// Returns from a manager sub-dialog (rename / delete) to the manager.
	pub(crate) fn back_to_manager(&mut self, cx: &mut Context<Self>) {
		self.modal = ModalState::None;
		self.show_project_manager(cx);
	}

	/// Opens the platform save dialog for exporting the library row `uuid`
	/// (the suggested name is `<project>.ove`; the format follows the
	/// extension the user picks).
	pub(crate) fn open_manager_export(&mut self, uuid: String, cx: &mut Context<Self>) {
		let name = self
			.manager_selected_name(cx)
			.filter(|n| !n.is_empty())
			.unwrap_or_else(|| "project".to_string());
		self.pending_export = Some(uuid);
		let receiver = cx.prompt_for_new_path(&PathBuf::from("."), Some(&format!("{name}.ove")));
		cx.spawn(async move |this, cx| {
			if let Ok(Ok(Some(path))) = receiver.await {
				let _ = this.update(cx, |this, cx| {
					this.on_file_paths(FileAction::ExportProject, vec![path], cx);
				});
			}
		})
		.detach();
	}

	// -----------------------------------------------------------------------
	// Modal dialogs
	// -----------------------------------------------------------------------
}
