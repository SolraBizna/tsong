use crate::*;
use gtk::{
    prelude::*,
    Align,
    BoxBuilder,
    ButtonBoxBuilder, ButtonBoxStyle,
    Button, ButtonBuilder,
    CellRendererText,
    Entry, EntryBuilder,
    LabelBuilder,
    ListStore,
    Orientation,
    PolicyType,
    ScrolledWindowBuilder,
    SeparatorBuilder,
    ToolButton, ToolButtonBuilder,
    TreeView, TreeViewBuilder, TreeViewColumn, TreePath,
    Window, WindowBuilder, WindowType,
};
use glib::{
    Type
};
use std::{
    cell::RefCell,
    rc::{Rc,Weak},
};

// TODO: this should be fluent...
const PLAYLIST_CODE_TOOLTIP: &str =
    "Enter playlist code here, e.g.:\n\
     \n\
     album:contains \"Moonlight\" or artist:starts_with \"The Answer\"\n\
     \n\
     Leave empty to include only manually added songs.";

pub struct Controller {
    window: Window,
    me: Option<Weak<RefCell<Controller>>>,
    parent: Weak<RefCell<super::Controller>>,
    active_playlist: Option<PlaylistRef>,
    column_tag_cell: CellRendererText,
    column_tag_column: TreeViewColumn,
    columns_model: ListStore,
    columns_view: TreeView,
    delete_column_button: ToolButton,
    new_column_button: ToolButton,
    playlist_code: Entry,
    apply_button: Button,
    cancel_button: Button,
    ok_button: Button,
}

impl Controller {
    pub fn new(parent: Weak<RefCell<super::Controller>>)
    -> Rc<RefCell<Controller>> {
        let window = WindowBuilder::new()
            .name("playlist_editor").type_(WindowType::Toplevel)
            .title("Tsong - Playlist Editor").build();
        let big_box = BoxBuilder::new()
            .name("playlist_editor").orientation(Orientation::Vertical)
            .spacing(4).build();
        window.add(&big_box);
        // The playlist code:
        // TODO: make this a monospace font?
        big_box.add(&LabelBuilder::new()
                                .label("Playlist Rule:")
                                .halign(Align::Start).build());
        let playlist_code = EntryBuilder::new().hexpand(true)
            .placeholder_text("Manually added songs only")
            .tooltip_text(PLAYLIST_CODE_TOOLTIP)
            .build();
        big_box.add(&playlist_code);
        // The columns
        big_box.add(&LabelBuilder::new()
                                .label("Shown Metadata:")
                                .halign(Align::Start).build());
        let columns_window = ScrolledWindowBuilder::new()
            .name("columns")
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let columns_view = TreeViewBuilder::new()
            .headers_visible(false).reorderable(true).build();
        let column_tag_column = TreeViewColumn::new();
        let column_tag_cell = CellRendererText::new();
        column_tag_cell.set_property("editable", &true)
            .expect("couldn't make column cell editable");
        column_tag_column.pack_start(&column_tag_cell, true);
        column_tag_column.add_attribute(&column_tag_cell, "text", 0);
        columns_view.append_column(&column_tag_column);
        columns_window.add(&columns_view);
        big_box.add(&columns_window);
        let column_button_box = ButtonBoxBuilder::new()
            .layout_style(ButtonBoxStyle::Expand)
            .build();
        let delete_column_button
            = ToolButtonBuilder::new().icon_name("list-remove").build();
        delete_column_button.set_sensitive(false);
        column_button_box.add(&delete_column_button);
        let new_column_button
            = ToolButtonBuilder::new().icon_name("list-add").build();
        column_button_box.add(&new_column_button);
        big_box.add(&column_button_box);
        // The buttons
        big_box.pack_start(&SeparatorBuilder::new()
                                       .orientation(Orientation::Horizontal)
                                       .build(), false, true, 6);
        let buttons_box = BoxBuilder::new()
            .orientation(Orientation::Horizontal).build();
        let button_box = ButtonBoxBuilder::new()
            .spacing(6).build();
        let cancel_button = ButtonBuilder::new()
            .label("_Cancel").use_underline(true).build();
        buttons_box.pack_start(&cancel_button, false, true, 0);
        let apply_button = ButtonBuilder::new()
            .label("_Apply").use_underline(true).build();
        button_box.pack_end(&apply_button, false, true, 0);
        let ok_button = ButtonBuilder::new()
            .label("Save & Cl_ose").use_underline(true).build();
        ok_button.get_style_context().add_class("suggested-action");
        button_box.pack_end(&ok_button, false, true, 0);
        buttons_box.pack_end(&button_box, false, true, 0);
        big_box.add(&buttons_box);
        let ret = Rc::new(RefCell::new(Controller {
            window,
            parent, columns_model: ListStore::new(&[Type::String, Type::U32]),
            delete_column_button, new_column_button, column_tag_column,
            columns_view, apply_button, cancel_button, ok_button,
            column_tag_cell, playlist_code, active_playlist: None,
            me: None
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
        self.playlist_code.set_text("");
        let parent = self.parent.upgrade()?;
        parent.try_borrow_mut().ok()?.closed_playlist_edit();
        None
    }
    pub fn show(&mut self) {
        if !self.window.is_visible() {
            self.populate();
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
    fn populate(&mut self) {
        let playlist = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => return,
        };
        let playlist = playlist.read().unwrap();
        self.playlist_code.set_text(playlist.get_rule_code());
        self.check_playlist_code();
        for column in playlist.get_columns() {
            self.columns_model.insert_with_values(None, &[0, 1],
                                                  &[&column.tag.to_value(),
                                                    &column.width.to_value()]);
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
        let wo = self.columns_view.get_cursor().0?;
        self.columns_model.get_iter(&wo)
            .map(|x| self.columns_model.remove(&x));
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
}
