use crate::*;
use log::{warn, error, trace};
use gtk::{
    prelude::*,
    Align,
    BoxBuilder,
    ButtonBoxBuilder, ButtonBoxStyle,
    Button, ButtonBuilder,
    ButtonsType,
    CellRendererText,
    CellRendererToggle,
    DestDefaults,
    DialogFlags,
    Entry, EntryBuilder,
    LabelBuilder,
    ListStore,
    MessageDialog, MessageType,
    Notebook, NotebookBuilder,
    Orientation,
    PolicyType,
    ResponseType,
    ScrolledWindowBuilder,
    SelectionMode,
    SeparatorBuilder,
    TargetEntry, TargetFlags,
    TreeStore,
    TreeView, TreeViewBuilder, TreeViewColumn, TreeIter, TreePath,
    TreeRowReference,
    Widget,
    Window, WindowBuilder, WindowType,
};
use glib::{
    Type,
};
use gdk::{
    DragAction,
    ModifierType,
};
use lazy_static::lazy_static;
use std::{
    collections::{BTreeMap, HashMap},
    cell::RefCell,
    fmt::Write,
    rc::{Rc, Weak},
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
};

const TSONG_FILE_MIMETYPE: &str = "application/x-os-path";
const TSONG_FILE_TYPE: u32 = 3;

// TODO: this should be fluent...
const PLAYLIST_CODE_TOOLTIP: &str =
    "Enter playlist code here, e.g.:\n\
     \n\
     album:contains \"Moonlight\" or artist:starts_with \"The Answer\"\n\
     \n\
     See the readme for more examples and a more detailed description. \
     Leave empty to include only manually added songs.";

pub struct Controller {
    window: Window,
    me: Option<Weak<RefCell<Controller>>>,
    parent: Weak<RefCell<super::Controller>>,
    active_playlist: Option<PlaylistRef>,
    selected_songs: Vec<LogicalSongRef>,
    column_tag_cell: CellRendererText,
    column_tag_column: TreeViewColumn,
    columns_model: ListStore,
    columns_view: TreeView,
    delete_column_button: Button,
    new_column_button: Button,
    metadata_model: ListStore,
    metadata_view: TreeView,
    files_model: TreeStore,
    files_view: TreeView,
    meta_key_cell: CellRendererText,
    meta_key_column: TreeViewColumn,
    meta_value_cell: CellRendererText,
    meta_modified_cell: CellRendererToggle,
    /// The metadata values as they currently exist. `Some("...")` = all
    /// selected songs have this value for this key. `None` = at least one song
    /// has this key, but not all songs have the same value for it.
    meta_orig: BTreeMap<String, Option<String>>,
    /// Maps metadata keys that already existed to their renamed names. This is
    /// applied BEFORE `meta_edits`.
    meta_renames: BTreeMap<String, String>,
    /// Maps metadata keys that may or may not exist to their new values. Non-
    /// empty string = the value is set. Empty string = the key is deleted.
    meta_edits: BTreeMap<String, String>,
    delete_meta_button: Button,
    // meta_script_button: Button,
    reimport_all_meta_button: Button,
    reimport_selected_meta_button: Button,
    new_meta_button: Button,
    notebook: Notebook,
    playlist_notebook: Notebook,
    song_notebook: Notebook,
    columns_page: u32,
    meta_page: u32,
    playlist_page: u32,
    song_page: u32,
    playlist_code: Entry,
    apply_button: Button,
    cancel_button: Button,
    revert_button: Button,
    ok_button: Button,
    song_meta_update_tx: mpsc::Sender<SongID>,
    /// Whether a "background script" is currently running.
    ///
    /// This only transitions from `false` to `true` in the main thread, and
    /// from `true` to `false` in a side thread.
    script_in_progress: Arc<AtomicBool>,
}

const META_COLUMN_TYPES: &[Type] = &[Type::String, Type::String, Type::U32,
                                     Type::Bool, Type::String, Type::Bool];
const META_KEY_COLUMN: u32 = 0;
const META_VALUE_COLUMN: u32 = 1;
const META_ROW_WEIGHT_COLUMN: u32 = 2;
const META_MODIFIED_COLUMN: u32 = 3;
const META_ORIG_KEY_COLUMN: u32 = 4;
const META_DELETED_COLUMN: u32 = 5;

lazy_static! {
    static ref FILES_COLUMN_TYPES: [Type; 5]
        = [Type::String, Type::Bool, Type::Bool,
           super::SONG_ID_TYPE,
           *super::FILE_ID_TYPE];
}
const FILE_NAME_COLUMN: u32 = 0;
const FILE_IS_ACTUALLY_CHECKBOX_COLUMN: u32 = 1;
const FILE_IS_SENSITIVE_COLUMN: u32 = 2;
const FILE_SONG_ID_COLUMN: u32 = 3;
const FILE_FILE_ID_COLUMN: u32 = 4;

// TODO: i18n
const MULTIPLE_VALUES: &str = "(multiple values)";
const DELETED_VALUE: &str = "(delete)";
// Currently only used when a value is newly created and hasn't been filled in
// yet. In future, may also be used for certain "privileged" keys like "title"
// or "artist".
const EMPTY_VALUE: &str = "";

impl Controller {
    pub fn new(parent: Weak<RefCell<super::Controller>>,
               song_meta_update_tx: mpsc::Sender<SongID>)
    -> Rc<RefCell<Controller>> {
        let window = WindowBuilder::new()
            .name("editor").type_(WindowType::Toplevel)
            .title("Tsong - Editor").build();
        let big_box = BoxBuilder::new()
            .name("editor").orientation(Orientation::Vertical)
            .build();
        window.add(&big_box);
        let notebook = NotebookBuilder::new().name("editor")
            .show_border(false).build();
        big_box.add(&notebook);
        let playlist_notebook = NotebookBuilder::new().name("playlist")
            .show_border(false).build();
        let playlist_page = notebook.append_page::<_, Widget>(&playlist_notebook, None);
        notebook.set_tab_label_text(&playlist_notebook, "Playlist");
        let song_notebook = NotebookBuilder::new().name("song")
            .show_border(false).build();
        let song_page = notebook.append_page::<_, Widget>(&song_notebook, None);
        notebook.set_tab_label_text(&song_notebook, "Song");
        let columns_box = BoxBuilder::new()
            .name("playlist_columns")
            .orientation(Orientation::Vertical).spacing(4).build();
        let columns_page = playlist_notebook.append_page::<_, Widget>(&columns_box, None);
        playlist_notebook.set_tab_label_text(&columns_box, "Columns");
        let sort_box = BoxBuilder::new()
            .name("playlist_sort")
            .orientation(Orientation::Vertical).spacing(4).build();
        sort_box.add(&LabelBuilder::new().label("Not implemented yet. For \
                                                 now, change the sort by \
                                                 clicking on the column \
                                                 headings.").build());
        playlist_notebook.append_page::<_, Widget>(&sort_box, None);
        playlist_notebook.set_tab_label_text(&sort_box, "Sort");
        let rule_box = BoxBuilder::new()
            .name("playlist_rules")
            .orientation(Orientation::Vertical).spacing(4).build();
        playlist_notebook.append_page::<_, Widget>(&rule_box, None);
        playlist_notebook.set_tab_label_text(&rule_box, "Rules");
        let meta_box = BoxBuilder::new()
            .name("song_meta")
            .orientation(Orientation::Vertical).spacing(4).build();
        let meta_page = song_notebook.append_page::<_, Widget>(&meta_box, None);
        song_notebook.set_tab_label_text(&meta_box, "Metadata");
        let files_box = BoxBuilder::new()
            .name("files")
            .orientation(Orientation::Vertical).spacing(4).build();
        song_notebook.append_page::<_, Widget>(&files_box, None);
        song_notebook.set_tab_label_text(&files_box, "Files");
        // The playlist code:
        // TODO: make this a monospace font?
        rule_box.add(&LabelBuilder::new()
                        .label("Lua code:")
                        .halign(Align::Start).build());
        let playlist_code = EntryBuilder::new().hexpand(true)
            .placeholder_text("Manually added songs only")
            .tooltip_text(PLAYLIST_CODE_TOOLTIP)
            .build();
        rule_box.add(&playlist_code);
        // The columns
        let columns_window = ScrolledWindowBuilder::new()
            .name("columns")
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let columns_view = TreeViewBuilder::new()
            .tooltip_text("List of metadata tags that will appear in the \
                           playlist view. You probably at least want \
                           \"title\" or \"track#\" to be present here.\n\n\
                           Click and drag to reorder, or use the buttons \
                           below to add and remove elements.")
            .headers_visible(false).reorderable(true).build();
        columns_view.get_selection().set_mode(SelectionMode::Multiple);
        let column_tag_column = TreeViewColumn::new();
        let column_tag_cell = CellRendererText::new();
        column_tag_cell.set_property("editable", &true)
            .expect("couldn't make column cell editable");
        column_tag_column.pack_start(&column_tag_cell, true);
        column_tag_column.add_attribute(&column_tag_cell, "text", 0);
        columns_view.append_column(&column_tag_column);
        columns_window.add(&columns_view);
        columns_box.add(&columns_window);
        let column_button_box = ButtonBoxBuilder::new()
            .layout_style(ButtonBoxStyle::Expand)
            .build();
        let delete_column_button = ButtonBuilder::new()
            .tooltip_text("Delete the selected metadata tag from the list of \
                           tags to display in the playlist view.")
            .build();
        delete_column_button.set_sensitive(false);
        column_button_box.add(&delete_column_button);
        super::set_icon(&delete_column_button, "tsong-remove");
        let new_column_button = ButtonBuilder::new()
            .tooltip_text("Add a new metadata tag to the list of tags to \
                           display in the playlist view.")
            .build();
        column_button_box.add(&new_column_button);
        columns_box.add(&column_button_box);
        super::set_icon(&new_column_button, "tsong-add");
        // The song metadata
        let metadata_model = ListStore::new(META_COLUMN_TYPES);
        let metadata_window = ScrolledWindowBuilder::new()
            .name("metadata")
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let metadata_view = TreeViewBuilder::new()
            .model(&metadata_model).headers_visible(true).reorderable(true)
            .build();
        metadata_view.get_selection().set_mode(SelectionMode::Multiple);
        let tvc = TreeViewColumn::new();
        let meta_modified_cell = CellRendererToggle::new();
        meta_modified_cell.set_alignment(0.5, 0.5);
        tvc.pack_start(&meta_modified_cell, true);
        tvc.add_attribute(&meta_modified_cell, "active", META_MODIFIED_COLUMN as i32);
        metadata_view.append_column(&tvc);
        let meta_key_column = TreeViewColumn::new();
        let meta_key_cell = CellRendererText::new();
        meta_key_column.set_title("Key");
        meta_key_column.set_fixed_width(100);
        meta_key_column.set_resizable(true);
        meta_key_cell.set_property("editable", &true)
            .expect("couldn't make column cell editable");
        meta_key_column.pack_start(&meta_key_cell, true);
        meta_key_column.add_attribute(&meta_key_cell, "text",
                                      META_KEY_COLUMN as i32);
        meta_key_column.add_attribute(&meta_key_cell, "weight",
                                      META_ROW_WEIGHT_COLUMN as i32);
        meta_key_column.add_attribute(&meta_key_cell, "strikethrough",
                                      META_DELETED_COLUMN as i32);
        metadata_view.append_column(&meta_key_column);
        let meta_value_column = TreeViewColumn::new();
        let meta_value_cell = CellRendererText::new();
        meta_value_column.set_title("Value");
        meta_value_cell.set_property("editable", &true)
            .expect("couldn't make column cell editable");
        meta_value_column.pack_start(&meta_value_cell, true);
        meta_value_column.add_attribute(&meta_value_cell, "text",
                                        META_VALUE_COLUMN as i32);
        meta_value_column.add_attribute(&meta_value_cell, "weight",
                                        META_ROW_WEIGHT_COLUMN as i32);
        metadata_view.append_column(&meta_value_column);
        metadata_window.add(&metadata_view);
        meta_box.add(&metadata_window);
        let metadata_button_box = ButtonBoxBuilder::new()
            .layout_style(ButtonBoxStyle::Expand)
            .build();
        let delete_meta_button = ButtonBuilder::new()
            .tooltip_text("Delete the selected metadata tag from the selected \
                           songs' metadata.")
            .build();
        delete_meta_button.set_sensitive(false);
        metadata_button_box.add(&delete_meta_button);
        super::set_icon(&delete_meta_button, "tsong-remove");
        // Hide unimplemented feature
        /*
        let meta_script_button = ButtonBuilder::new()
            .label("Run _Lua Script…").use_underline(true).build();
        meta_script_button.set_sensitive(false);
        metadata_button_box.add(&meta_script_button);
         */
        let reimport_all_meta_button = ButtonBuilder::new()
            .tooltip_text("Re-import all metadata for the selected songs, \
                           replacing any Tsong-specific metadata they may \
                           have with the original, on-disk metadata from the \
                           original files.")
            .label("_Re-import All").use_underline(true).build();
        reimport_all_meta_button.set_sensitive(false);
        metadata_button_box.add(&reimport_all_meta_button);
        let reimport_selected_meta_button = ButtonBuilder::new()
            .tooltip_text("Re-import each selected metadata tag's original, \
                           on-disk value (if any) to the selected songs. Any \
                           selected metadata tags that aren't found in the \
                           original metadata will be erased.")
            .label("Re-import _Selected").use_underline(true).build();
        reimport_selected_meta_button.set_sensitive(false);
        metadata_button_box.add(&reimport_selected_meta_button);
        let new_meta_button = ButtonBuilder::new()
            .tooltip_text("Add a new metadata tag to the selected songs' \
                           metadata.")
            .build();
        new_meta_button.set_sensitive(false);
        metadata_button_box.add(&new_meta_button);
        meta_box.add(&metadata_button_box);
        super::set_icon(&new_meta_button, "tsong-add");
        // Song files
        let files_model = TreeStore::new(&FILES_COLUMN_TYPES[..]);
        let files_window = ScrolledWindowBuilder::new()
            .name("files")
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let files_view = TreeViewBuilder::new()
            .headers_visible(false)
            .model(&files_model).build();
        let tvc = TreeViewColumn::new();
        let import_metadata_cell = CellRendererToggle::new();
        import_metadata_cell.set_alignment(0.5, 0.5);
        tvc.pack_start(&import_metadata_cell, true);
        tvc.add_attribute(&import_metadata_cell, "visible", FILE_IS_ACTUALLY_CHECKBOX_COLUMN as i32);
        tvc.add_attribute(&import_metadata_cell, "sensitive", FILE_IS_SENSITIVE_COLUMN as i32);
        let name_or_path_cell = CellRendererText::new();
        tvc.pack_start(&name_or_path_cell, true);
        tvc.add_attribute(&name_or_path_cell, "text", FILE_NAME_COLUMN as i32);
        tvc.add_attribute(&name_or_path_cell, "sensitive", FILE_IS_SENSITIVE_COLUMN as i32);
        files_view.append_column(&tvc);
        let file_type = TargetEntry::new(TSONG_FILE_MIMETYPE,
                                         TargetFlags::SAME_APP
                                         | TargetFlags::SAME_WIDGET,
                                         TSONG_FILE_TYPE);
        files_view.drag_source_set(ModifierType::BUTTON1_MASK,
                                   &[file_type.clone()],
                                   DragAction::MOVE);
        files_view.drag_dest_set(DestDefaults::empty(),
                                 &[file_type.clone()],
                                 DragAction::MOVE);
        files_view.connect_drag_begin(|_widget, context| {
            trace!("files_view begins drag");
            context.drag_set_icon_name("tsong-dragged-file", 0, 0);
        });
        files_view.connect_drag_end(|_widget, _context| {
            trace!("files_view ends drag");
        });
        files_view.connect_drag_failed(|_widget, _context, why| {
            trace!("files_view failed drag ({:?})", why);
            Inhibit(false)
        });
        files_view.get_selection()
            .set_select_function(Some(Box::new(|_selection, _model, path, selected| {
                // Something that's selected can always be deselected.
                if selected { return true }
                // Only files can be selected.
                else { return path.get_depth() > 1 }
            })));
        files_window.add(&files_view);
        files_box.add(&files_window);
        // The buttons
        big_box.pack_start(&SeparatorBuilder::new()
                           .orientation(Orientation::Horizontal)
                           .build(), false, true, 0);
        let buttons_box = BoxBuilder::new()
            .name("buttons").spacing(6)
            .orientation(Orientation::Horizontal).build();
        let button_box = ButtonBoxBuilder::new()
            .spacing(6).build();
        let cancel_button = ButtonBuilder::new()
            .tooltip_text("Close this window without saving your changes.")
            .label("_Cancel").use_underline(true).build();
        buttons_box.pack_start(&cancel_button, false, true, 0);
        let revert_button = ButtonBuilder::new()
            .tooltip_text("Revert your changes.")
            .label("Rever_t").use_underline(true).build();
        buttons_box.pack_start(&revert_button, false, true, 0);
        let apply_button = ButtonBuilder::new()
            .tooltip_text("Apply your changes.")
            .label("_Apply").use_underline(true).build();
        button_box.pack_end(&apply_button, false, true, 0);
        let ok_button = ButtonBuilder::new()
            .tooltip_text("Close this window and apply your changes.")
            .label("Save & Cl_ose").use_underline(true).build();
        ok_button.get_style_context().add_class("suggested-action");
        button_box.pack_end(&ok_button, false, true, 0);
        buttons_box.pack_end(&button_box, false, true, 0);
        big_box.add(&buttons_box);
        let ret = Rc::new(RefCell::new(Controller {
            window, notebook, playlist_notebook, song_notebook,
            columns_page, meta_page, song_page, playlist_page,
            parent, columns_model: ListStore::new(&[Type::String, Type::U32]),
            delete_column_button, new_column_button, column_tag_column,
            delete_meta_button, reimport_all_meta_button,
            reimport_selected_meta_button, new_meta_button,
            columns_view, apply_button, cancel_button, ok_button,
            revert_button, // meta_script_button,
            meta_key_cell, meta_value_cell, meta_key_column,meta_modified_cell,
            meta_orig: BTreeMap::new(),
            meta_edits: BTreeMap::new(), meta_renames: BTreeMap::new(),
            column_tag_cell, playlist_code, active_playlist: None,
            metadata_model, metadata_view, files_model, files_view,
            script_in_progress: Arc::new(AtomicBool::new(false)),
            selected_songs: Vec::new(), me: None,
            song_meta_update_tx,
        }));
        let mut this = ret.borrow_mut();
        this.me = Some(Rc::downgrade(&ret));
        this.columns_view.set_model(Some(&this.columns_model));
        let controller = ret.clone();
        this.playlist_code.connect_property_text_notify(move |_| {
            let _ = controller.try_borrow()
                .map(|x| x.check_playlist_code());
        });
        let controller = ret.clone();
        this.window.connect_delete_event(move |window, _| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.cleanup());
            window.hide_on_delete()
        });
        let controller = ret.clone();
        this.apply_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_apply());
        });
        let controller = ret.clone();
        this.cancel_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_cancel());
        });
        let controller = ret.clone();
        this.revert_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.populate());
        });
        let controller = ret.clone();
        this.ok_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_ok());
        });
        let controller = ret.clone();
        this.delete_column_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_delete_column());
        });
        let controller = ret.clone();
        this.new_column_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_new_column());
        });
        let controller = ret.clone();
        this.column_tag_cell.connect_edited(move |_, wo, nu| {
            let _ = controller.try_borrow()
                .map(|x| x.edited_column_tag(wo, nu));
        });
        let delete_column_button = this.delete_column_button.clone();
        this.columns_view.connect_cursor_changed(move |columns_view| {
            // this doesn't reference Controller because we *want* it to update
            // automatically, even when we caused the change
            delete_column_button.set_sensitive
                (columns_view.get_cursor().0.is_some())
        });
        let controller = ret.clone();
        this.meta_key_cell.connect_edited(move |_, wo, nu| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.edited_meta_key(wo, nu));
        });
        let controller = ret.clone();
        this.meta_value_cell.connect_edited(move |_, wo, nu| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.edited_meta_value(wo, nu));
        });
        let controller = ret.clone();
        this.meta_modified_cell.connect_toggled(move |_, wo| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.try_cancel_edit(wo));
        });
        let controller = ret.clone();
        this.new_meta_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_new_meta());
        });
        let controller = ret.clone();
        let window = this.window.clone();
        let metadata_view = this.metadata_view.clone();
        this.reimport_selected_meta_button.connect_clicked(move |_| {
            if controller.borrow().maybe_show_script_wait_dialog() {
                return;
            }
            let selection = metadata_view.get_selection();
            let (wo_list, model) = selection.get_selected_rows();
            let model: &ListStore = model.downcast_ref().unwrap();
            let keys_to_reimport: Vec<String> = wo_list.into_iter()
                .filter_map(|wo| model.get_iter(&wo))
                .filter_map(|iter| model.get_value(&iter,
                                                   META_KEY_COLUMN as i32)
                            .get().ok()?)
                .collect();
            if keys_to_reimport.is_empty() {
                // we weren't supposed to be clickable in the first place
                return;
            }
            let dirty = {
                let controller = controller.borrow_mut();
                !(controller.meta_renames.is_empty()
                  && controller.meta_edits.is_empty())
            };
            let dialog = if dirty {
                MessageDialog::new(Some(&window),
                                   DialogFlags::MODAL,
                                   MessageType::Error,
                                   ButtonsType::Cancel,
                                   "Please apply your changes before re-\
                                    importing specific metadata.")
            }
            else {
                MessageDialog::new(Some(&window),
                                   DialogFlags::MODAL,
                                   MessageType::Warning,
                                   ButtonsType::OkCancel,
                                   "Are you sure you want to attempt to \
                                    replace all selected metadata with values \
                                    from the original? (Metadata missing from \
                                    the original will be lost!)")
            };
            let result = dialog.run();
            dialog.close();
            if result == ResponseType::Cancel { return }
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.reimport_selected_meta(keys_to_reimport));
        });
        let controller = ret.clone();
        let window = this.window.clone();
        this.reimport_all_meta_button.connect_clicked(move |_| {
            if controller.borrow().maybe_show_script_wait_dialog() {
                return;
            }
            let dialog = MessageDialog::new(Some(&window),
                                            DialogFlags::MODAL,
                                            MessageType::Warning,
                                            ButtonsType::OkCancel,
                                            "Are you sure you want to \
                                             re-import the original metadata? \
                                             ALL CUSTOM METADATA WILL BE \
                                             LOST!");
            // technically, we're lying. if their custom import.lua doesn't
            // destroy outmeta like the default one does, metadata will stick
            // around.
            let result = dialog.run();
            dialog.close();
            if result == ResponseType::Cancel { return }
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.reimport_all_meta());
        });
        let controller = ret.clone();
        this.delete_meta_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_delete_meta());
        });
        let delete_meta_button = this.delete_meta_button.clone();
        let reimport_selected_meta_button = this.reimport_selected_meta_button
            .clone();
        this.metadata_view.connect_cursor_changed(move |metadata_view| {
            // this doesn't reference Controller because we *want* it to update
            // automatically, even when we caused the change
            delete_meta_button.set_sensitive
                (metadata_view.get_cursor().0.is_some());
            reimport_selected_meta_button.set_sensitive
                (metadata_view.get_cursor().0.is_some());
        });
        drop(this);
        ret
    }
    fn clicked_apply(&mut self) -> Option<()> {
        let playlist_code = match self.check_playlist_code() {
            Some(x) => x,
            None => {
                self.playlist_code.grab_focus();
                return None;
            },
        };
        let mut columns = Vec::new();
        self.columns_model.foreach(|model, _path, iter| {
            let tag = model.get_value(&iter, 0);
            let width = model.get_value(&iter, 1);
            match (tag.get(), width.get()) {
                (Ok(Some(tag)), Ok(Some(width))) =>
                    columns.push(playlist::Column { tag, width }),
                _ => (),
            }
            false
        });
        let parent = self.parent.upgrade()?;
        parent.try_borrow_mut().ok()?
            .edit_playlist(playlist_code, columns);
        if !self.meta_renames.is_empty() || !self.meta_edits.is_empty() {
            for song_ref in self.selected_songs.iter() {
                self.apply_meta_edits(song_ref);
            }
        }
        // This will get called automatically when the main UI notices we've
        // changed some metadata. Bonus: It won't if we've been called by
        // clicking "Save & Close" and our window got closed!
        //self.populate_song();
        None
    }
    fn clicked_cancel(&mut self) {
        self.window.close();
        self.cleanup();
    }
    fn clicked_ok(&mut self) {
        self.clicked_apply();
        self.window.close();
        self.cleanup();
    }
    fn cleanup(&mut self) -> Option<()> {
        self.columns_model.clear();
        self.metadata_model.clear();
        self.files_model.clear();
        self.playlist_code.set_text("");
        self.meta_orig.clear();
        self.meta_renames.clear();
        self.meta_edits.clear();
        let parent = self.parent.upgrade()?;
        parent.try_borrow_mut().ok()?.closed_edit();
        None
    }
    pub fn show(&mut self) {
        if !self.window.is_visible() {
            self.populate();
            if self.selected_songs.len() == 0 {
                self.notebook.set_current_page(Some(self.playlist_page));
            }
            else {
                self.notebook.set_current_page(Some(self.song_page));
            }
            self.playlist_notebook.set_current_page(Some(self.columns_page));
            self.song_notebook.set_current_page(Some(self.meta_page));
            self.window.show_all();
        }
        else {
            self.window.present();
        }
    }
    pub fn unshow(&mut self) {
        self.window.close();
        self.cleanup();
    }
    pub fn activate_playlist(&mut self, playlist: Option<PlaylistRef>) {
        self.active_playlist = playlist;
        if !self.window.is_visible() { return }
        self.populate();
    }
    pub fn set_selected_songs(&mut self, songs: &[SongID]) {
        self.selected_songs.clear();
        self.selected_songs.reserve(songs.len());
        for song_id in songs.iter() {
            logical::get_song_by_song_id(*song_id)
                .map(|x| self.selected_songs.push(x));
        }
        if self.window.is_visible() { self.populate_song() }
        self.reimport_all_meta_button.set_sensitive(self.selected_songs.len() !=0);
        self.new_meta_button.set_sensitive(self.selected_songs.len() != 0);
        //self.meta_script_button.set_sensitive(self.selected_songs.len() != 0);
    }
    fn populate(&mut self) {
        let playlist_ref = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => return,
        };
        let playlist = playlist_ref.read().unwrap();
        self.playlist_code.set_text(playlist.get_rule_code());
        self.check_playlist_code();
        self.columns_model.clear();
        for column in playlist.get_columns() {
            self.columns_model.insert_with_values(None, &[0, 1],
                                                  &[&column.tag.to_value(),
                                                    &column.width.to_value()]);
        }
        drop(playlist);
        self.populate_song();
    }
    fn populate_song(&mut self) {
        self.metadata_model.clear();
        self.files_model.clear();
        self.meta_orig.clear();
        self.meta_renames.clear();
        self.meta_edits.clear();
        for song_ref in self.selected_songs.iter() {
            let song = song_ref.read().unwrap();
            let metadata = song.get_metadata();
            for (key, value) in metadata.iter() {
                if key == "duration" || key == "song_id" { continue }
                // TODO: clean this up? decide to keep it?
                if value.len() == 0 { continue }
                use std::collections::btree_map::Entry;
                match self.meta_orig.entry(key.to_owned()) {
                    Entry::Vacant(x) => {
                        x.insert(Some(value.to_owned()));
                    },
                    Entry::Occupied(x) => {
                        let all_value = x.into_mut();
                        match all_value {
                            Some(x) if x == value => (),
                            Some(_) => *all_value = None,
                            None => (),
                        }
                    },
                }
            }
            // while we're here, populate the files table!
            let song_id = song.get_id();
            let song_id_value = super::song_id_to_value(song_id);
            let song_iter = self.files_model.insert_with_values
                (None, None,
                 &[FILE_NAME_COLUMN, FILE_IS_SENSITIVE_COLUMN,
                   FILE_SONG_ID_COLUMN],
                 &[&format!("{}", *song), &false, &song_id_value]);
            let files = song.get_physical_files();
            for file_id in files {
                let mut name = String::new();
                match physical::get_file_by_id(file_id) {
                    None => write!(name, "MISSING FILE").unwrap(),
                    Some(file_ref) => {
                        let file = file_ref.read().unwrap();
                        let paths = file.get_absolute_paths();
                        if paths.len() > 0 {
                            for path in paths {
                                write!(name, "{}\n",
                                       path.to_string_lossy()).unwrap();
                            }
                            name.pop(); // pop off trailing \n
                        }
                        else {
                            write!(name, "<{}>", file_id).unwrap();
                        }
                    },
                }
                self.files_model.insert_with_values
                    (Some(&song_iter), None,
                     &[FILE_NAME_COLUMN,
                       FILE_IS_SENSITIVE_COLUMN,
                       FILE_SONG_ID_COLUMN,
                       FILE_FILE_ID_COLUMN],
                     &[&name,
                       &true, &song_id_value,
                       &super::file_id_to_value(file_id)]);
            }
        }
        for song_ref in self.selected_songs.iter() {
            let song = song_ref.read().unwrap();
            let metadata = song.get_metadata();
            for (key, value) in self.meta_orig.iter_mut() {
                if !metadata.contains_key(key) {
                    *value = None;
                }
            }
        }
        let mut sorted: Vec<&str> = self.meta_orig.keys().map(String::as_str).collect();
        sorted.sort();
        for key in sorted.iter() {
            let iter = self.metadata_model.append();
            self.metadata_model.set_value(&iter, META_KEY_COLUMN,
                                          &key.to_value());
            self.metadata_model.set_value(&iter, META_ORIG_KEY_COLUMN,
                                          &key.to_value());
            self.metadata_model.set_value(&iter,
                                          META_ROW_WEIGHT_COLUMN,
                                          &super::INACTIVE_WEIGHT
                                          .to_value());
            match self.meta_orig.get(*key) {
                Some(Some(x)) => {
                    self.metadata_model.set_value(&iter, META_VALUE_COLUMN,
                                                  &x.to_value());
                },
                _ => {
                    self.metadata_model.set_value(&iter, META_VALUE_COLUMN,
                                                  &MULTIPLE_VALUES.to_value());
                },
            }
        }
    }
    fn check_playlist_code(&self) -> Option<String> {
        let value = self.playlist_code.get_text();
        let code_as_string: String = value.into();
        let style_context = self.playlist_code.get_style_context();
        match Playlist::syntax_check_rule_code(&code_as_string) {
            Err(x) => {
                style_context.add_class("error");
                self.playlist_code.set_tooltip_text(Some(&x));
                None
            },
            Ok(_) => {
                style_context.remove_class("error");
                self.playlist_code
                    .set_tooltip_text(Some(PLAYLIST_CODE_TOOLTIP));
                Some(code_as_string)
            }
        }
    }
    fn clicked_delete_column(&mut self) -> Option<()> {
        let selection = self.columns_view.get_selection();
        let (wo_list, model) = selection.get_selected_rows();
        let row_list: Vec<TreeRowReference> = wo_list.into_iter()
            .filter_map(|x| TreeRowReference::new(&model, &x))
            .collect();
        for row in row_list.iter() {
            self.columns_model.remove(&row.get_path()
                                      .and_then(|x| model.get_iter(&x))
                                      .unwrap());
        }
        None
    }
    fn clicked_new_column(&mut self) {
        let it = self.columns_model.insert_with_values
            (None, &[0, 1],
             &[&"".to_value(),
               &playlist::DEFAULT_COLUMN_WIDTH.to_value()]);
        match self.columns_model.get_path(&it) {
            Some(wo) =>
                self.columns_view
                .set_cursor_on_cell(&wo,
                                    Some(&self.column_tag_column),
                                    Some(&self.column_tag_cell),
                                    true),
            _ => (),
        }
    }
    fn edited_column_tag(&self, wo: TreePath, nu: &str) -> Option<()> {
        let iter = self.columns_model.get_iter(&wo)?;
        self.columns_model.set_value(&iter, 0, &nu.to_value());
        None
    }
    fn update_modified_for_row(&mut self, iter: &TreeIter) -> Option<bool> {
        let orig_key: String
            = self.metadata_model.get_value(&iter, META_ORIG_KEY_COLUMN as i32)
            .get().ok()??; // if None, we didn't need to update this
        let modified = if self.meta_renames.get(&orig_key).is_some() {
            true
        }
        else {
            let orig_value = self.meta_orig.get(&orig_key).unwrap();
            let neo_value = self.meta_edits.get(&orig_key);
            match (orig_value, neo_value) {
                // value is not being modified
                (_, None) => false,
                // originally had multiple values, now either has a single
                // value or is deleted
                (None, Some(_)) => true,
                // originally had a single value, now may have a different
                // value
                (Some(x), Some(y)) => x != y,
            }
        };
        self.metadata_model.set_value(&iter, META_MODIFIED_COLUMN,
                                      &modified.to_value());
        self.metadata_model.set_value(&iter, META_ROW_WEIGHT_COLUMN,
                                      &if modified { super::INACTIVE_WEIGHT }
                                      else { super::INACTIVE_WEIGHT }
                                      .to_value());
        Some(modified)
    }
    /// Find out if there's already another metadata key with that index (in
    /// the edited form)
    fn already_has_meta_key(&self, key: &str, skip: Option<&TreePath>)
    -> bool {
        let mut dupe = false;
        self.metadata_model.foreach(|model, path, iter| {
            if Some(path) == skip { return false }
            let that_key: String =
                model.get_value(&iter, META_KEY_COLUMN as i32)
                .get().ok().unwrap().unwrap();
            if that_key == key {
                dupe = true;
                return true
            }
            false
        });
        dupe
    }
    fn edited_meta_key(&mut self, wo: TreePath, nu: &str) -> Option<()> {
        let iter = self.metadata_model.get_iter(&wo)?;
        let prev_key: Option<String>
            = self.metadata_model.get_value(&iter, META_KEY_COLUMN as i32)
            .get().ok()?;
        // Reject the edit if the name is invalid.
        if nu == "" || nu == "duration" || nu == "song_id" {
            // (If the edit is rejected, and this is a newly-created row that
            // has not yet had a valid value, just delete it.)
            if prev_key.is_some() {
                self.metadata_model.remove(&iter);
            }
            return None
        }
        let dupe = self.already_has_meta_key(&nu, Some(&wo));
        // If there is already another key with the same name, reject the edit.
        if dupe {
            // (see above)
            if prev_key.is_some() {
                self.metadata_model.remove(&iter);
            }
            return None
        }
        // If there's not, let's rename it!
        let orig_key: Option<String>
            = self.metadata_model.get_value(&iter, META_ORIG_KEY_COLUMN as i32)
            .get().ok()?;
        if let Some(orig_key) = orig_key {
            let modified = nu != orig_key;
            if modified {
                self.meta_renames.insert(orig_key, nu.to_owned());
                // definitely modified
                self.metadata_model.set_value(&iter, META_MODIFIED_COLUMN,
                                              &true.to_value());
                self.metadata_model.set_value(&iter, META_ROW_WEIGHT_COLUMN,
                                              &super::ACTIVE_WEIGHT
                                              .to_value());
            }
            else {
                self.meta_renames.remove(&orig_key);
                // maybe modified
                self.update_modified_for_row(&iter);
            }
        }
        else {
            // we don't have an original key, so we should not touch
            // `meta_renames`, and we need not update the modified flag
        }
        self.metadata_model.set_value(&iter, META_KEY_COLUMN, &nu.to_value());
        if let Some(prev_key) = prev_key.as_ref() {
            let prev_edit = self.meta_edits.remove(prev_key);
            if let Some(prev_edit) = prev_edit {
                self.meta_edits.insert(nu.to_owned(), prev_edit);
            }
        }
        // as a convenience, if this is a newly created key, try going directly
        // to editing the value
        // ...or not (this triggers a GTK+ assertion failure)
        /*
        if prev_key.is_none() {
            self.metadata_view
                .set_cursor_on_cell(&wo,
                                    Some(&self.meta_value_column),
                                    Some(&self.meta_value_cell),
                                    true);
        }
         */
        None
    }
    fn edited_meta_value(&mut self, wo: TreePath, nu: &str) -> Option<()> {
        let iter = self.metadata_model.get_iter(&wo)?;
        let key: String
            = self.metadata_model.get_value(&iter, META_KEY_COLUMN as i32)
            .get().ok()??;
        self.meta_edits.insert(key, nu.to_owned());
        if nu == "" {
            self.metadata_model.set_value(&iter,
                                          META_VALUE_COLUMN,
                                          &DELETED_VALUE.to_value());
            self.metadata_model.set_value(&iter,
                                          META_MODIFIED_COLUMN,
                                          &true.to_value());
            self.metadata_model.set_value(&iter,
                                          META_DELETED_COLUMN,
                                          &false.to_value());
            self.metadata_model.set_value(&iter, META_ROW_WEIGHT_COLUMN,
                                          &super::ACTIVE_WEIGHT
                                          .to_value());
        }
        else {
            self.metadata_model.set_value(&iter,
                                          META_VALUE_COLUMN,
                                          &nu.to_value());
            self.metadata_model.set_value(&iter,
                                          META_DELETED_COLUMN,
                                          &false.to_value());
            self.update_modified_for_row(&iter);
        }
        None
    }
    fn try_cancel_edit(&mut self, wo: TreePath) -> Option<()> {
        let iter = self.metadata_model.get_iter(&wo)?;
        let key: String
            = self.metadata_model.get_value(&iter, META_KEY_COLUMN as i32)
            .get().ok()??;
        let orig_key: Option<String>
            = self.metadata_model.get_value(&iter, META_ORIG_KEY_COLUMN as i32)
            .get().ok()?;
        let orig_key = match orig_key {
            Some(orig_key) => orig_key,
            None => {
                // No original to restore. Delete the whole darn row!
                self.metadata_model.remove(&iter);
                self.meta_edits.remove(&key);
                return None
            },
        };
        let dupe = self.already_has_meta_key(&orig_key, Some(&wo));
        if dupe { return None }
        self.meta_edits.remove(&key);
        self.meta_renames.remove(&orig_key);
        self.metadata_model.set_value(&iter, META_KEY_COLUMN,
                                      &orig_key.to_value());
        match self.meta_orig.get(&orig_key) {
            Some(Some(x)) => {
                self.metadata_model.set_value(&iter, META_VALUE_COLUMN,
                                              &x.to_value());
            },
            _ => {
                self.metadata_model.set_value(&iter, META_VALUE_COLUMN,
                                              &MULTIPLE_VALUES.to_value());
            },
        }
        self.metadata_model.set_value(&iter, META_MODIFIED_COLUMN,
                                      &false.to_value());
        self.metadata_model.set_value(&iter, META_DELETED_COLUMN,
                                      &false.to_value());
        self.metadata_model.set_value(&iter, META_ROW_WEIGHT_COLUMN,
                                      &super::INACTIVE_WEIGHT
                                      .to_value());
        None
    }
    fn apply_meta_edits(&self, song_ref: &LogicalSongRef) {
        let mut dirty = false;
        let mut song = song_ref.write().unwrap();
        let mut metadata = song.get_metadata().clone();
        // We have to put all the renamed values in a separate hat before we
        // apply them, because otherwise there might be some clobbering.
        let mut renamed = HashMap::with_capacity(self.meta_renames.len());
        for (from, to) in self.meta_renames.iter() {
            if let Some(value) = metadata.remove(from) {
                dirty = true;
                renamed.insert(to.clone(), value);
            }
        }
        // Okay, now put all the renamed things back in...
        for (key, value) in renamed.into_iter() {
            metadata.insert(key, value);
        }
        // And then apply all edits.
        for (key, value) in self.meta_edits.iter() {
            if metadata.get(key) != Some(&value) {
                metadata.insert(key.clone(), value.clone());
                dirty = true;
            }
        }
        // Okay!
        if dirty && song.set_metadata(metadata) {
            let _ = self.song_meta_update_tx.send(song.get_id());
        }
    }
    fn clicked_new_meta(&mut self) {
        let it = self.metadata_model.insert_with_values
            (None, &[META_VALUE_COLUMN, META_ROW_WEIGHT_COLUMN,
                     META_MODIFIED_COLUMN],
             &[&EMPTY_VALUE.to_value(), &super::ACTIVE_WEIGHT.to_value(),
               &true.to_value()]);
        match self.metadata_model.get_path(&it) {
            Some(wo) =>
                self.metadata_view
                .set_cursor_on_cell(&wo,
                                    Some(&self.meta_key_column),
                                    Some(&self.meta_key_cell),
                                    true),
            _ => (),
        }
    }
    fn clicked_delete_meta(&mut self) -> Option<()> {
        let selection = self.metadata_view.get_selection();
        let (wo_list, model) = selection.get_selected_rows();
        let model: &ListStore = model.downcast_ref().unwrap();
        let row_list: Vec<TreeRowReference> = wo_list.into_iter()
            .filter_map(|x| TreeRowReference::new(model, &x))
            .collect();
        for row in row_list.iter() {
            let path = match row.get_path() {
                Some(x) => x,
                None => continue,
            };
            let iter = match model.get_iter(&path) {
                Some(x) => x,
                None => continue,
            };
            let orig_key: Option<String> = self.metadata_model
                .get_value(&iter, META_ORIG_KEY_COLUMN as i32)
                .get().ok()?;
            let current_key: Option<String> = self.metadata_model
                .get_value(&iter, META_KEY_COLUMN as i32)
                .get().ok()?;
            match (orig_key, current_key) {
                (Some(_orig_key), Some(current_key)) => {
                    self.meta_edits.insert(current_key, String::new());
                    model.set_value(&iter, META_VALUE_COLUMN,
                                    &DELETED_VALUE.to_value());
                    model.set_value(&iter, META_DELETED_COLUMN,
                                    &true.to_value());
                    model.set_value(&iter, META_MODIFIED_COLUMN,
                                    &true.to_value());
                    self.metadata_model.set_value(&iter,
                                                  META_ROW_WEIGHT_COLUMN,
                                                  &super::ACTIVE_WEIGHT
                                                  .to_value());
                },
                (orig_key, current_key) => {
                    if let Some(orig_key) = orig_key {
                        self.meta_renames.remove(&orig_key);
                    }
                    if let Some(current_key) = current_key {
                        self.meta_edits.remove(&current_key);
                    }
                    self.metadata_model.remove(&iter);
                }
            }
        }
        None
    }
    fn kickoff_script<T: 'static + FnOnce() + Send>(&mut self, func: T) {
        let script_in_progress = self.script_in_progress.clone();
        script_in_progress.store(true, Ordering::Relaxed);
        match self.parent.upgrade() {
            Some(parent) => match parent.try_borrow() {
                Ok(parent) => parent.force_spinner_start(),
                _ => (),
            },
            _ => (),
        }
        std::thread::Builder::new().name("Background Script".to_string())
            .spawn(move || {
                func();
                script_in_progress.store(false, Ordering::Relaxed);
            }).expect("Couldn't find background thread");
    }
    fn reimport_all_meta(&mut self) {
        // TODO: here, and in reimport_selected_meta, allow to choose which
        // file to import metadata from
        let selected_songs = self.selected_songs.clone();
        let song_meta_update_tx = self.song_meta_update_tx.clone();
        self.kickoff_script(move || {
            for song_ref in selected_songs.iter() {
                let mut song = song_ref.write().unwrap();
                let file = match song.get_physical_files().iter()
                    .filter_map(physical::get_file_by_id)
                    .next() {
                        Some(file) => file,
                        None => {
                            drop(song);
                            warn!("Song {:?} couldn't be reimported \
                                   because it has no physical files...?",
                                  song_ref);
                            continue
                        },
                    };
                let file = file.read().unwrap();
                match song.import_metadata(&*file, None) {
                    Ok(false) => (),
                    Ok(true) => {
                        let _ = song_meta_update_tx.send(song.get_id());
                    },
                    Err(x) => {
                        drop(song);
                        error!("While importing metadata for song {:?}:\n\
                                {}", song_ref, x);
                        continue
                    },
                }
            }
        });
    }
    fn reimport_selected_meta(&mut self, keys_to_import: Vec<String>) {
        let selected_songs = self.selected_songs.clone();
        let song_meta_update_tx = self.song_meta_update_tx.clone();
        self.kickoff_script(move || {
            for song_ref in selected_songs.iter() {
                let mut song = song_ref.write().unwrap();
                let file = match song.get_physical_files().iter()
                    .filter_map(physical::get_file_by_id)
                    .next() {
                        Some(file) => file,
                        None => {
                            drop(song);
                            warn!("Song {:?} couldn't be reimported \
                                   because it has no physical files...?",
                                  song_ref);
                            continue
                        },
                    };
                let file = file.read().unwrap();
                let imported = match song.get_imported_metadata(&*file, None) {
                    Ok(x) => x,
                    Err(x) => {
                        drop(song);
                        error!("Error importing metadata for song {:?}:\n\
                                {}", song_ref, x);
                        continue
                    },
                };
                let mut new_metadata = song.get_metadata().clone();
                for key in keys_to_import.iter() {
                    new_metadata.remove(key);
                    if let Some(value) = imported.get(key) {
                        new_metadata.insert(key.clone(), value.clone());
                    }
                }
                if song.set_metadata(new_metadata) {
                    let _ = song_meta_update_tx.send(song.get_id());
                }
            }
        });
    }
    fn maybe_show_script_wait_dialog(&self) -> bool {
        if !self.script_is_in_progress() { return false }
        let dialog = MessageDialog::new(Some(&self.window),
                                        DialogFlags::MODAL,
                                        MessageType::Error,
                                        ButtonsType::Cancel,
                                        "Please wait for the previous batch \
                                         operation to complete before \
                                         starting another.");
        let _ = dialog.run();
        dialog.close();
        true
    }
    pub fn script_is_in_progress(&self) -> bool {
        self.script_in_progress.load(Ordering::Relaxed)
    }
}
