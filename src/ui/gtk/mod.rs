use crate::*;
use log::{warn, error, trace};
use fuse_rust::Fuse;
use gtk::{
    prelude::*,
    Adjustment,
    Align,
    Allocation,
    Application,
    ApplicationWindow,
    BoxBuilder,
    Button, ButtonBuilder, ButtonBoxBuilder, ButtonBoxStyle,
    ButtonsType,
    CellRendererText,
    Container,
    DestDefaults,
    DialogFlags,
    Entry,
    Grid, GridBuilder,
    IconSize, IconTheme,
    Image,
    Label, LabelBuilder,
    ListStore,
    MessageDialog, MessageType,
    Orientation,
    Overlay, OverlayBuilder,
    PolicyType,
    ReliefStyle,
    ResponseType,
    Scale, ScaleBuilder,
    ScrolledWindowBuilder,
    SelectionData,
    SelectionMode,
    SeparatorBuilder,
    Spinner, SpinnerBuilder,
    StyleContext,
    TargetEntry, TargetFlags,
    ToggleButton, ToggleButtonBuilder,
    TreeIter, TreePath, TreeStore, TreeRowReference,
    TreeModel, TreeModelFlags,
    TreeView, TreeViewBuilder, TreeViewColumn, TreeViewDropPosition,
    Widget,
};
use gdk::{
    Atom,
    DragAction, DragContext,
    Geometry,
    Gravity,
    ModifierType,
    Screen,
    WindowHints,
};
use glib::{
    types::Type,
    source::{SourceId, source_remove, timeout_add_local},
    Value,
};
use gio::prelude::*;
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::HashSet,
    convert::TryInto,
    rc::{Rc,Weak},
    sync::{RwLockReadGuard, mpsc},
};

use anyhow::anyhow;

mod settings;
mod playlist_edit;
mod errors_window;
mod scrp;
use scrp::*;

const INACTIVE_WEIGHT: u32 = 400; // normal weight
const ACTIVE_WEIGHT: u32 = 800; // bold
const TSONG_SONGS_MIMETYPE: &str = "application/x-tsong-songs";
const TSONG_PLAYLISTS_MIMETYPE: &str = "application/x-tsong-playlists";
const TSONG_SONGS_TYPE: u32 = 1;
const TSONG_PLAYLISTS_TYPE: u32 = 2;

const PLAYLIST_ID_COLUMN: u32 = 0;
const PLAYLIST_NAME_COLUMN: u32 = 1;
const PLAYLIST_WEIGHT_COLUMN: u32 = 2;

const SONG_ID_COLUMN: u32 = 0;
const SONG_WEIGHT_COLUMN: u32 = 1;
const SONG_INDEX_COLUMN: u32 = 2;
const SONG_IS_MANUAL_COLUMN: u32 = 3;
const SONG_FIRST_META_COLUMN: u32 = 4;

pub struct Controller {
    active_playlist: Option<PlaylistRef>,
    control_box: gtk::Box,
    delete_playlist_button: Button,
    last_built_playlist: Option<PlaylistRef>,
    new_playlist_button: Button,
    next_button: Button,
    osd: Label,
    play_button: Button,
    playlist_model: Option<ListStore>,
    playlist_name_cell: CellRendererText,
    playlist_name_column: TreeViewColumn,
    playlist_stats: Label,
    playlist_view: TreeView,
    playlists_model: TreeStore,
    playlists_view: TreeView,
    playmode_button: ToggleButton,
    playlist_edit_button: ToggleButton,
    errors_button: ToggleButton,
    prev_button: Button,
    rollup_button: Button,
    rollup_grid: Grid,
    settings_button: ToggleButton,
    shuffle_button: ToggleButton,
    volume_scale: Scale,
    volume_label: Label,
    window: ApplicationWindow,
    playlist_generation: GenerationValue,
    errors_generation: GenerationValue,
    scan_spinner: Spinner,
    remote: Option<Remote>,
    remote_time: f64,
    last_active_playlist: Option<(TreeIter,PlaylistRef)>,
    last_active_song: Option<(Option<TreeIter>,LogicalSongRef)>,
    scan_thread: ScanThread,
    rolled_down_height: i32,
    settings_controller: Option<Rc<RefCell<settings::Controller>>>,
    playlist_edit_controller: Option<Rc<RefCell<playlist_edit::Controller>>>,
    errors_controller: Option<Rc<RefCell<errors_window::Controller>>>,
    periodic_timer: Option<SourceId>,
    volume_changed: bool,
    me: Option<Weak<RefCell<Controller>>>,
    song_meta_update_rx: mpsc::Receiver<SongID>,
}

impl Controller {
    pub fn new(application: &Application) -> Rc<RefCell<Controller>> {
        let mut scan_thread = ScanThread::new();
        scan_thread.rescan(prefs::get_music_paths())
            .expect("Couldn't start the initial music scan!");
        let icon_theme = IconTheme::get_default().unwrap();
        if let Ok(path) = std::env::var("TSONG_ICON_PATH") {
            icon_theme.append_search_path(&path);
        }
        let resource_bytes = glib::Bytes::from_static
            (include_bytes!("tsong.gresource"));
        let resource = gio::Resource::from_data(&resource_bytes).unwrap();
        gio::resources_register(&resource);
        icon_theme.add_resource_path("/name/bizna/tsong/icons");
        let window = ApplicationWindow::new(application);
        window.set_title("Tsong");
        window.set_default_size(640, 460);
        let provider = gtk::CssProvider::new();
        provider.load_from_data(include_bytes!("css.css")).unwrap();
        StyleContext::add_provider_for_screen(&Screen::get_default().unwrap(),
                                              &provider, 750);
        let outer_box = BoxBuilder::new().orientation(Orientation::Vertical)
            .build();
        window.add(&outer_box);
        // The outer box is divided into two things.
        // One, a fixed-height row that contains the playback controls:
        let control_box = BoxBuilder::new()
            .name("controls")
            .spacing(4).hexpand(true).build();
        outer_box.add(&control_box);
        // Two, taking up the rest of the window, a variable-height box that
        // contains the playlist controls:
        let rollup_grid = GridBuilder::new()
            .expand(true).build();
        // So, the playback controls...
        // Button to bring up the settings window:
        let settings_button = ToggleButtonBuilder::new()
            .tooltip_text("Open a window where you can change settings such \
                           as audio device and song locations.")
            .name("settings").build();
        control_button_add(&control_box, &settings_button, &["popup"]);
        // Button to go back to the previous song in the playlist:
        let prev_button = ButtonBuilder::new()
            .tooltip_text("Jump to the previous song in the playlist.")
            .name("prev").build();
        control_button_add(&control_box, &prev_button, &["circular"]);
        // Explicit play/pause button
        let play_button = ButtonBuilder::new()
            .tooltip_text("Start or pause playback.")
            .name("playpause").build();
        control_button_add(&control_box, &play_button, &["circular"]);
        // Button to go to the next song in the playlist:
        let next_button = ButtonBuilder::new()
            .tooltip_text("Jump to the next song in the playlist.")
            .name("next").build();
        control_button_add(&control_box, &next_button, &["circular"]);
        // Osd widget!
        let osd = LabelBuilder::new()
            .name("osd")
            .hexpand(true).build();
        control_box.add(&osd);
        // Volume slider:
        let volume_overlay = OverlayBuilder::new()
            .name("volume").expand(false).build();
        let volume_box = BoxBuilder::new()
            .name("fake").expand(false).build();
        let quiet_icon = Image::from_icon_name(Some("tsong-volume-quiet"),
                                               IconSize::LargeToolbar);
        quiet_icon.set_widget_name("quiet");
        quiet_icon.set_halign(Align::Start);
        quiet_icon.set_valign(Align::Center);
        let loud_icon = Image::from_icon_name(Some("tsong-volume-loud"),
                                               IconSize::LargeToolbar);
        loud_icon.set_widget_name("loud");
        loud_icon.set_halign(Align::End);
        loud_icon.set_valign(Align::Center);
        let volume_scale = ScaleBuilder::new()
            .has_origin(true)
            .draw_value(false)
            .show_fill_level(true)
            .fill_level(100.0)
            .restrict_to_fill_level(false)
            .adjustment(&Adjustment::new(prefs::get_volume() as f64,
                                         0.0, 200.0, 1.0, 10.0, 10.0))
            .tooltip_text("Adjust playback volume. Note that volumes above \
                           100% may result in distortion.")
            .build();
        let volume_label = LabelBuilder::new()
            .halign(Align::Center).valign(Align::Center).build();
        set_volume_label(&volume_scale, &volume_label);
        volume_overlay.add(&volume_box);
        volume_overlay.add_overlay(&quiet_icon);
        volume_overlay.add_overlay(&volume_label);
        volume_overlay.add_overlay(&loud_icon);
        volume_overlay.add_overlay(&volume_scale);
        control_box.add(&volume_overlay);
        let volume_label_clone = volume_label.clone();
        volume_scale.connect_value_changed(move |volume_scale| {
            set_volume_label(volume_scale, &volume_label_clone)
        });
        // Button to "roll up" the playlist box:
        let rollup_button = ButtonBuilder::new()
            .tooltip_text("Toggle between the full interface and the compact \
                           interface.")
            .name("rollup").build();
        control_button_add(&control_box, &rollup_button, &["toggle"]);
        // That's the end of the playback controls.
        // Now, the playlists!
        let separator = SeparatorBuilder::new()
            .orientation(Orientation::Vertical).build();
        rollup_grid.attach(&separator, 1, 0, 1, 2);
        let playlists_box = BoxBuilder::new()
            .orientation(Orientation::Vertical).build();
        let playlists_window = ScrolledWindowBuilder::new()
            .name("playlists")
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let playlists_view = TreeViewBuilder::new()
            .headers_visible(false).build();
        playlists_view.set_search_column(1);
        playlists_view.get_selection().set_mode(SelectionMode::Multiple);
        playlists_window.add(&playlists_view);
        playlists_box.add(&playlists_window);
        rollup_grid.attach(&playlists_box, 0, 0, 1, 1);
        let playlist_button_box = ButtonBoxBuilder::new()
            .layout_style(ButtonBoxStyle::Expand)
            .build();
        let delete_playlist_button = ButtonBuilder::new()
            .tooltip_markup("Delete the selected playlist(s) forever. (This \
                             will never delete a <i>song</i>.)")
            .build();
        playlist_button_box.add(&delete_playlist_button);
        let new_playlist_button = ButtonBuilder::new()
            .tooltip_text("Create a new, empty playlist.")
            .build();
        playlist_button_box.add(&new_playlist_button);
        rollup_grid.attach(&playlist_button_box, 0, 1, 1, 1);
        // and the play...list
        let playlist_window = ScrolledWindowBuilder::new()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true).hexpand(true)
            .build();
        let playlist_view = TreeViewBuilder::new().expand(true)
            .headers_visible(true).build();
        playlist_view.set_search_equal_func(playlist_search_func);
        playlist_view.get_selection().set_mode(SelectionMode::Multiple);
        playlist_window.add(&playlist_view);
        rollup_grid.attach(&playlist_window, 2, 0, 1, 1);
        // Playlist stats and controls
        let below_playlist_box = BoxBuilder::new()
            .name("below_playlist").orientation(Orientation::Horizontal)
            .build();
        // Playlist stats
        let bottom_overlay = Overlay::new();
        let playlist_stats = LabelBuilder::new()
            .name("playlist_stats").build();
        bottom_overlay.add(&playlist_stats);
        let scan_spinner = SpinnerBuilder::new().name("scan_spinner")
            .halign(Align::Start).valign(Align::Center).build();
        bottom_overlay.add_overlay(&scan_spinner);
        // and, just because...!
        let errors_button = ToggleButtonBuilder::new().name("errors")
            .halign(Align::End).valign(Align::Center).hexpand(false)
            .relief(ReliefStyle::None).build();
        bottom_overlay.add_overlay(&errors_button);
        errors_button.set_sensitive(false);
        below_playlist_box.pack_start(&bottom_overlay, true, true, 0);
        // buttons!
        let playlist_control_box = ButtonBoxBuilder::new()
            .name("meta").layout_style(ButtonBoxStyle::Expand)
            .homogeneous(false)
            .orientation(Orientation::Horizontal).build();
        // make the right edge merge with the window edge :)
        playlist_control_box.pack_end(&BoxBuilder::new().build(), false, false, 0);
        // Button to change shuffle mode:
        let shuffle_button = ToggleButtonBuilder::new()
            .tooltip_text("Toggle shuffle mode. When active, the playlist \
                           will be played back in a random order.")
            .name("shuffle").build();
        playlist_control_box.pack_start(&shuffle_button, false, false, 0);
        // Button to change loop mode:
        let playmode_button = ToggleButtonBuilder::new()
            .tooltip_text("Toggle looping mode. Can either loop a single \
                           song, loop an entire playlist, or never loop.")
            .name("playmode").build();
        playlist_control_box.pack_start(&playmode_button, false, false, 0);
        // Button to edit playlist settings:
        let playlist_edit_button = ToggleButtonBuilder::new()
            .tooltip_text("Open a window where you can edit properties of \
                           this playlist, or of the selected song(s).")
            .name("edit_playlist").label("Edit").build();
        playlist_control_box.pack_end(&playlist_edit_button, false, false, 0);
        below_playlist_box.pack_start(&playlist_control_box, false, false, 0);
        rollup_grid.attach(&below_playlist_box, 2, 1, 1, 1);
        outer_box.add(&rollup_grid);
        // done setting up the widgets, time to bind everything to the
        // controller
        let manual_song_type = TargetEntry::new(TSONG_SONGS_MIMETYPE,
                                                TargetFlags::SAME_APP
                                                | TargetFlags::OTHER_WIDGET,
                                                TSONG_SONGS_TYPE);
        let playlist_type = TargetEntry::new(TSONG_PLAYLISTS_MIMETYPE,
                                             TargetFlags::SAME_APP
                                             | TargetFlags::SAME_WIDGET,
                                                TSONG_PLAYLISTS_TYPE);
        playlist_view.connect_drag_begin(|_widget, context| {
            trace!("playlist_view begins drag");
            context.drag_set_icon_name("tsong-dragged-playlist", 0, 0);
        });
        playlist_view.connect_drag_end(|_widget, _context| {
            trace!("playlist_view ends drag");
        });
        playlist_view.connect_drag_failed(|_widget, _context, why| {
            trace!("playlist_view failed drag ({:?})", why);
            Inhibit(false)
        });
        playlists_view.connect_drag_begin(|_widget, context| {
            trace!("playlists_view begins drag");
            context.drag_set_icon_name("tsong-dragged-song", 0, 0);
        });
        playlists_view.connect_drag_end(|_widget, _context| {
            trace!("playlists_view ends drag");
        });
        playlists_view.connect_drag_failed(|_widget, _context, why| {
            trace!("playlists_view failed drag ({:?})", why);
            Inhibit(false)
        });
        playlist_view.drag_source_set(ModifierType::BUTTON1_MASK,
                                      &[manual_song_type.clone()],
                                      DragAction::LINK);
        playlists_view.drag_source_set(ModifierType::BUTTON1_MASK,
                                      &[playlist_type.clone()],
                                       DragAction::MOVE);
        playlists_view.drag_dest_set(DestDefaults::empty(),
                                     &[manual_song_type.clone(),
                                       playlist_type.clone()],
                                     DragAction::LINK|DragAction::MOVE);
        let tsong_songs_mimetype_atom = Atom::intern(TSONG_SONGS_MIMETYPE);
        let tsong_playlists_mimetype_atom = Atom::intern(TSONG_PLAYLISTS_MIMETYPE);
        playlist_view.connect_drag_data_get(
            move |playlist_view, _context, data, _info, _timestamp| {
                trace!("playlist_view drag data get!");
                let selection = playlist_view.get_selection();
                let (wo_list, model) = selection.get_selected_rows();
                let mut selected_songs = Vec::new();
                for wo in wo_list.iter() {
                    model.get_iter(wo)
                        .map(|x| model.get_value(&x, SONG_ID_COLUMN as i32))
                        .map(|x| {
                            let id = value_to_song_id(x).unwrap().as_inner();
                            selected_songs.extend_from_slice
                                (&id.to_ne_bytes()[..])
                        });
                }
                data.set(&tsong_songs_mimetype_atom, 8, &selected_songs[..]);
            });
        playlists_view.connect_drag_data_get(
            move |playlists_view, _context, data, _info, _timestamp| {
                trace!("playlists_view drag data get!");
                let selection = playlists_view.get_selection();
                let (wo_list, model) = selection.get_selected_rows();
                let mut selected_playlists = Vec::new();
                for wo in wo_list.iter() {
                    model.get_iter(wo)
                        .map(|x| model.get_value(&x,
                                                 PLAYLIST_ID_COLUMN as i32))
                        .map(|x| {
                            let id = value_to_playlist_id(x).unwrap()
                                .as_inner();
                            selected_playlists.extend_from_slice
                                (&id.to_ne_bytes()[..])
                        });
                }
                data.set(&tsong_playlists_mimetype_atom,
                         8, &selected_playlists[..]);
            });
        playlists_view.connect_drag_motion(
            move|playlists_view, context, x, y, time| {
                trace!("playlists_view drag motion {}, {}!", x, y);
                match check_drag_onto_playlist
                    (playlists_view, context, x, y, time,
                     &tsong_songs_mimetype_atom,
                     &tsong_playlists_mimetype_atom) {
                        Some((wo, pos, action, _target)) => {
                            context.drag_status(action, time);
                            playlists_view.set_drag_dest_row(Some(&wo), pos);
                            Inhibit(true)
                        },
                        None => {
                            let pos = TreeViewDropPosition::Before;
                            context.drag_status(DragAction::empty(), time);
                            playlists_view.set_drag_dest_row(None, pos);
                            Inhibit(false)
                        },
                    }
            });
        playlists_view.connect_drag_drop(
            move|playlists_view, context, x, y, time| {
                trace!("playlists_view drag drop {}, {}!", x, y);
                match check_drag_onto_playlist
                    (playlists_view, context, x, y, time,
                     &tsong_songs_mimetype_atom,
                     &tsong_playlists_mimetype_atom) {
                        Some((wo, pos, action, target)) => {
                            trace!("  Drag OK!");
                            context.drag_status(action, time);
                            playlists_view.set_drag_dest_row(Some(&wo), pos);
                            playlists_view.drag_get_data(context, target,
                                                         time);
                            Inhibit(true)
                        },
                        None => {
                            trace!("  Drag not OK!");
                            let pos = TreeViewDropPosition::Before;
                            context.drag_status(DragAction::empty(), time);
                            playlists_view.set_drag_dest_row(None, pos);
                            Inhibit(false)
                        },
                    }
            });
        let playlist_model = None;
        let (playlists_model, _, neu_active_playlist)
            = build_playlists_model(&[]);
        let last_active_playlist = neu_active_playlist;
        let playlist_name_column = TreeViewColumn::new();
        let playlist_name_cell = CellRendererText::new();
        playlist_name_cell.set_property("editable", &true)
            .expect("couldn't make playlist name cell editable");
        playlist_name_column.pack_start(&playlist_name_cell, true);
        playlist_name_column.add_attribute(&playlist_name_cell, "text", 1);
        playlist_name_column.add_attribute(&playlist_name_cell, "weight", 2);
        // I'd love to do this in CSS...
        // (ignore errors because this is not critical to functionality)
        // (fun fact! if this is an f32 instead of an f64, it breaks!)
        let _ = playlist_name_cell.set_property("scale", &0.80);
        // now, icons!
        set_icon(&settings_button, "tsong-settings");
        set_icon(&prev_button, "tsong-prev");
        set_icon(&next_button, "tsong-next");
        set_icon(&play_button, "tsong-pause");
        set_icon(&rollup_button, "tsong-rollup");
        set_icon(&shuffle_button, "tsong-shuffle");
        set_icon(&playmode_button, "tsong-loop");
        set_icon(&new_playlist_button, "tsong-add");
        set_icon(&delete_playlist_button, "tsong-remove");
        set_icon(&errors_button, "tsong-errors");
        let (song_meta_update_tx, song_meta_update_rx) = mpsc::channel();
        let nu = Rc::new(RefCell::new(Controller {
            rollup_button, settings_button, prev_button, next_button,
            shuffle_button, playmode_button, play_button, volume_scale,
            volume_label, playlists_view, playlist_view,
            playlists_model, playlist_model, playlist_stats, osd,
            scan_spinner, scan_thread, rollup_grid, control_box,
            new_playlist_button, delete_playlist_button,
            playlist_name_column, playlist_name_cell, window,
            playlist_edit_button, errors_button,
            remote: None, remote_time: -1.0,
            last_active_playlist, last_active_song: None,
            active_playlist: None, playlist_generation: Default::default(),
            errors_generation: Default::default(), errors_controller: None,
            last_built_playlist: None, me: None, settings_controller: None,
            playlist_edit_controller: None, rolled_down_height: 400,
            periodic_timer: None, volume_changed: false,
            song_meta_update_rx,
        }));
        // Throughout this application, we make use of a hack.
        // Each signal that depends on a Controller starts with an attempt to
        // mutably borrow the controller. If said attempt fails, that means
        // that the signal was raised by other code called from within the
        // controller, so we ignore the signal.
        let mut this = nu.borrow_mut();
        this.me = Some(Rc::downgrade(&nu));
        this.settings_controller = Some(settings::Controller::new(Rc::downgrade(&nu)));
        this.playlist_edit_controller = Some(playlist_edit::Controller::new(Rc::downgrade(&nu), song_meta_update_tx));
        this.errors_controller = Some(errors_window::Controller::new(Rc::downgrade(&nu)));
        this.remote = Some(Remote::new(Rc::downgrade(&nu)));
        this.delete_playlist_button
            .set_sensitive(this.delete_playlist_button_should_be_sensitive());
        this.playlists_view.append_column(&this.playlist_name_column);
        let controller = nu.clone();
        this.volume_scale.connect_value_changed(move |scale| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.update_volume(scale.get_value()));
        });
        this.prev_button.connect_clicked(|_| {
            playback::send_command(PlaybackCommand::Prev)
        });
        this.next_button.connect_clicked(|_| {
            playback::send_command(PlaybackCommand::Next)
        });
        let controller = nu.clone();
        this.rollup_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_rollup());
        });
        let controller = nu.clone();
        this.play_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_play());
        });
        this.playlists_view.set_model(Some(&this.playlists_model));
        let controller = nu.clone();
        this.playlist_name_cell.connect_edited(move |_, wo, nu| {
            let _ = controller.try_borrow()
                .map(|x| x.edited_playlist_name_in_view(wo, nu));
        });
        let controller = nu.clone();
        this.playlists_view.connect_cursor_changed(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.playlists_cursor_changed());
        });
        let controller = nu.clone();
        this.playlists_view.connect_drag_data_received(
            move|playlists_view, context, x, y, data, info, time| {
                match check_drag_onto_playlist
                    (playlists_view, context, x, y, time,
                     &tsong_songs_mimetype_atom,
                     &tsong_playlists_mimetype_atom) {
                        Some((wo, pos, _action, _target)) => {
                            match info {
                                TSONG_SONGS_TYPE => {
                                    let res = controller.try_borrow_mut()
                                        .map(|mut x| x.dragged_songs_onto_playlist_list(data, wo, pos, time)).unwrap_or((false, false));
context.drag_finish(res.0, res.1, time);
                                },
                                TSONG_PLAYLISTS_TYPE => {
                                    let res = controller.try_borrow_mut()
                                        .map(|mut x| x.dragged_playlists_in_playlist_list(data, wo, pos, time)).unwrap_or((false, false));
                                    context.drag_finish(res.0, res.1, time);
                                },
                                _ => {
                                    context.drag_finish(false, false, time);
                                },
                            }
                        },
                        None => {
                            // handle this without having to involve the
                            // controller
                            context.drag_finish(false, false, time);
                        },
                    }
            });
        let controller = nu.clone();
        this.playlist_view.connect_row_activated(move |_, wo, _| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.playlist_row_activated(wo));
        });
        let controller = nu.clone();
        this.shuffle_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_shuffle());
        });
        let controller = nu.clone();
         this.playmode_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_playmode());
        });
        let controller = nu.clone();
        this.new_playlist_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_new_playlist());
        });
        let controller = nu.clone();
        let window = this.window.clone();
        this.delete_playlist_button.connect_clicked(move |_| {
            let confirm = MessageDialog::new(Some(&window),
                                             DialogFlags::MODAL,
                                             MessageType::Warning,
                                             ButtonsType::OkCancel,
                                             "Are you sure you want to delete \
                                              the selected playlist?");
            let result = confirm.run();
            confirm.close();
            if result == ResponseType::Cancel { return }
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_delete_playlist());
        });
        let controller = nu.clone();
        this.window.connect_size_allocate(move |_, allocation| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.main_window_resized(allocation));
        });
        let controller = nu.clone();
        this.settings_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_settings());
        });
        let controller = nu.clone();
        this.errors_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_errors());
        });
        let controller = nu.clone();
        this.playlist_edit_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_playlist_edit());
        });
        let controller = nu.clone();
        this.window.connect_key_press_event(move |window, evt| {
            if window.activate_key(evt) { return Inhibit(true) }
            if !window.get_focus().map(|x| x.is::<Entry>()).unwrap_or(false) {
                let keyval = evt.get_keyval();
                use gdk::keys::constants as key;
                match keyval {
                    key::space => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_playpause());
                        return Inhibit(true)
                    },
                    key::Left => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_left());
                        return Inhibit(true)
                    },
                    key::Right => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_right());
                        return Inhibit(true)
                    },
                    // TODO: handle AudioForward and AudioRewind in another way
                    key::AudioCycleTrack | key::AudioForward
                    | key::AudioNext => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_next());
                        return Inhibit(true)
                    },
                    key::AudioRewind | key::AudioPrev => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_prev());
                        return Inhibit(true)
                    },
                    key::AudioLowerVolume => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_quieten());
                        return Inhibit(true)
                    },
                    key::AudioRaiseVolume => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_louden());
                        return Inhibit(true)
                    },
                    key::AudioMute => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_mute());
                        return Inhibit(true)
                    },
                    key::AudioPause => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_pause());
                        return Inhibit(true)
                    },
                    key::AudioPlay => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_play());
                        return Inhibit(true)
                    },
                    key::AudioStop => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_stop());
                        return Inhibit(true)
                    },
                    key::AudioRandomPlay => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_shuffle());
                        return Inhibit(true)
                    },
                    key::AudioRepeat => {
                        let _ = controller.try_borrow_mut()
                            .map(|mut x| x.remote_playmode());
                        return Inhibit(true)
                    },
                    _ => ()
                }
            }
            return Inhibit(false)
        });
        let controller = nu.clone();
        this.playlist_view.connect_key_press_event(move |_, evt| {
            let keyval = evt.get_keyval();
            use gdk::keys::constants as key;
            match keyval {
                key::Delete | key::BackSpace => {
                    let _ = controller.try_borrow_mut()
                        .map(|mut x| x.delete_selected_songs());
                    return Inhibit(true)
                },
                _ => (),
            }
            return Inhibit(false)
        });
        let controller = nu.clone();
        this.playlist_view.get_selection().connect_changed(move |_| {
            let _ = controller.try_borrow()
                .map(|x| x.update_selected_songs());
        });
        let controller = nu.clone();
        this.window.connect_delete_event(move |window, _| {
            let busy = controller.try_borrow_mut().unwrap()
                .background_task_in_progress();
            if busy {
                let confirm = MessageDialog::new
                    (Some(window), DialogFlags::MODAL, MessageType::Warning,
                     ButtonsType::OkCancel,
                     "There is a background task in progress. If you quit \
                      now, the task will be left half-finished.\n\n\
                      Are you sure you want to quit?");
                let result = confirm.run();
                confirm.close();
                Inhibit(result == ResponseType::Cancel)
            }
            else {
                Inhibit(false)
            }
        });
        this.activate_playlist_by_path(&TreePath::new_first());
        this.force_periodic();
        // okay, show the window and away we go
        this.window.show_all();
        // and now, this! (because show_all ruins it otherwise)
        this.errors_button.set_visible(false);
        drop(this);
        nu
    }
    pub fn rebuild_playlist_view(&mut self) {
        // Discard any song metadata updates that are queued, since we're
        // rebuilding the whole view.
        while let Ok(_) = self.song_meta_update_rx.try_recv() {}
        let songs_to_select = match self.last_built_playlist.as_ref() {
            Some(playlist) if Some(playlist) == self.active_playlist.as_ref()
            => {
                let (wo_list, model) = self.playlist_view.get_selection()
                    .get_selected_rows();
                let result: HashSet<SongID> = wo_list.iter()
                    .filter_map(|x| model.get_iter(&x))
                    .map(|x| model.get_value(&x, SONG_ID_COLUMN as i32))
                    .filter_map(value_to_song_id)
                    .collect();
                result
            },
            _ => HashSet::new(),
        };
        let active_song = playback::get_active_song();
        let active_song = active_song.as_ref().map(|x| &x.0);
        self.last_built_playlist = self.active_playlist.clone();
        // destroy existing columns
        // TODO: figure out why the sort indicator disappears
        while self.playlist_view.get_n_columns() > 0 {
            let n = (self.playlist_view.get_n_columns() - 1) as i32;
            let column = self.playlist_view.get_column(n).unwrap();
            column.set_sort_indicator(false);
            self.playlist_view.remove_column(&column);
        }
        let tvc = TreeViewColumn::new();
        tvc.set_title("#");
        tvc.set_clickable(false);
        tvc.set_fixed_width(50);
        tvc.set_sort_indicator(false);
        tvc.set_alignment(1.0);
        let cell = SensitiveCellRendererPixbuf::new();
        tvc.pack_start(&cell, true);
        cell.set_alignment(0.0, 0.5);
        let _ = cell.set_property("icon-name", &"tsong-delete-manual-song");
        tvc.add_attribute(&cell, "visible", SONG_IS_MANUAL_COLUMN as i32);
        tvc.add_attribute(&cell, "sensitive", SONG_IS_MANUAL_COLUMN as i32);
        let controller = Weak::upgrade(self.me.as_ref().unwrap())
            .unwrap();
        cell.connect_clicked(move |path| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.delete_manual_song(path));
            true
        });
        let cell = CellRendererText::new();
        tvc.pack_start(&cell, true);
        cell.set_alignment(1.0, 0.5);
        tvc.add_attribute(&cell, "text", SONG_INDEX_COLUMN as i32);
        tvc.add_attribute(&cell, "weight", SONG_WEIGHT_COLUMN as i32);
        self.playlist_view.append_column(&tvc);
        let playlist_ref = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => {
                self.playlist_model = None;
                self.playlist_view.set_model::<ListStore>(None);
                self.playlist_generation.destroy();
                self.shuffle_button.set_sensitive(false);
                self.shuffle_button.set_active(false);
                let _ = self.remote.as_ref().unwrap().set_is_shuffled(false);
                self.update_playmode_button();
                return
            },
        };
        let playlist = playlist_ref.maybe_refreshed();
        self.shuffle_button.set_sensitive(true);
        let is_shuffled = playlist.is_shuffled();
        self.shuffle_button.set_active(is_shuffled);
        let _ = self.remote.as_ref().unwrap().set_is_shuffled(is_shuffled);
        self.playlist_generation = playlist.get_playlist_generation();
        let mut types = Vec::with_capacity(playlist.get_columns().len() + 2);
        types.push(SONG_ID_TYPE); // Song ID
        types.push(Type::U32); // Weight of text
        types.push(Type::U32); // Index in playlist
        types.push(Type::Bool); // Whether the song was manually added
        for _ in playlist.get_columns() {
            types.push(Type::String); // Each metadata column...
        }
        // A bug in GTK+ prevents the built-in sort indicator from being useful
        // so let's just unplug all this code for now.
        /*
        let first_sort_by = playlist.get_sort_order().get(0);
         */
        let playlist_model = ListStore::new(&types[..]);
        let mut column_index: u32 = SONG_FIRST_META_COLUMN;
        for column in playlist.get_columns() {
            let tvc = TreeViewColumn::new();
            let cell = CellRendererText::new();
            tvc.set_title(&make_column_heading(&column.tag));
            tvc.set_clickable(true);
            tvc.set_resizable(true);
            tvc.set_fixed_width(column.width as i32);
            {
                let controller = Weak::upgrade(self.me.as_ref().unwrap())
                    .unwrap();
                let playlist_ref = playlist_ref.clone();
                let column_tag = column.tag.clone();
                tvc.connect_clicked(move |_| {
                    playlist_ref.write().unwrap().touched_heading(&column_tag);
                    controller.borrow_mut().rebuild_playlist_view();
                });
            }
            {
                let playlist_ref = playlist_ref.clone();
                let column_tag = column.tag.clone();
                tvc.connect_property_width_notify(move |tvc| {
                    playlist_ref.write().unwrap().resize_column(&column_tag,
                                                                tvc.get_width()
                                                                as u32);
                });
            }
            /*
            match first_sort_by {
                Some((tag,descending)) if tag == &column.tag => {
                    tvc.set_sort_indicator(true);
                    tvc.set_sort_order(if *descending { SortType::Descending }
                                       else { SortType::Ascending });
                },
                _ => {
                    tvc.set_sort_indicator(false);
                },
            };
             */
            tvc.pack_start(&cell, true);
            if column.tag == "duration" || column.tag == "year"
            || column.tag.ends_with("_number") || column.tag.ends_with("#") {
                cell.set_alignment(1.0, 0.5);
                // tvc.set_alignment(1.0);
            }
            tvc.add_attribute(&cell, "text", column_index as i32);
            tvc.add_attribute(&cell, "weight", SONG_WEIGHT_COLUMN as i32);
            // TODO: i18n this
            column_index += 1;
            self.playlist_view.append_column(&tvc);
        }
        let tvc = TreeViewColumn::new();
        tvc.set_title(""); // blank column to enforce sizes...
        self.playlist_view.append_column(&tvc);
        let mut song_index = 1;
        let mut total_duration = 0u32;
        self.last_active_song = active_song.map(|x| {
            (None, x.clone())
        });
        let manual_songs: HashSet<SongID>
            = playlist.get_manual_songs().iter().map(|x| *x).collect();
        // TODO: can we set the cursor and also select the proper other rows?
        let mut rows_to_select = Vec::new();
        for song_ref in playlist.get_songs() {
            let new_row = playlist_model.append();
            let song = song_ref.read().unwrap();
            if songs_to_select.contains(&song.get_id()) {
                playlist_model.get_path(&new_row)
                    .map(|x| rows_to_select.push(x));
            }
            playlist_model.set_value(&new_row, SONG_ID_COLUMN,
                                     &song_id_to_value(song.get_id()));
            let weight = if Some(song_ref) == active_song {
                // this is a doozy
                match &mut self.last_active_song {
                    Some(x) => x.0 = Some(new_row.clone()),
                    // can't be reached because active_song is non-None, and
                    // therefore last_active_song got set to Some above
                    _ => unreachable!(),
                }
                ACTIVE_WEIGHT
            }
            else { INACTIVE_WEIGHT };
            playlist_model.set_value(&new_row, SONG_WEIGHT_COLUMN,
                                     &weight.to_value());
            playlist_model.set_value(&new_row, SONG_INDEX_COLUMN,
                                     &song_index.to_value());
            song_index += 1;
            total_duration = total_duration.saturating_add(song.get_duration());
            self.emplace_metadata(&playlist_model, &new_row,
                                  playlist.get_columns(),
                                  &manual_songs, &*song);
        }
        self.playlist_view.set_model(Some(&playlist_model));
        self.playlist_model = Some(playlist_model);
        for wo in rows_to_select.into_iter() {
            self.playlist_view.get_selection().select_path(&wo);
        }
        match song_index-1 {
            0 => self.playlist_stats.set_label("No songs in playlist"),
            x => {
                let t = format!("{} song{} in playlist, total time {}",
                                x, if x == 1 { "" } else { "s" },
                                if total_duration == u32::MAX {
                                    "really really long".to_owned()
                                }
                                else {
                                    pretty_duration(total_duration)
                                });
                self.playlist_stats.set_label(&t);
            }
        }
        drop(playlist);
        self.update_playmode_button();
        self.update_selected_songs();
    }
    fn activate_playlist_by_path(&mut self, wo: &TreePath) {
        let id = match self.playlists_model.get_iter(wo)
            .map(|x| self.playlists_model.get_value(&x, PLAYLIST_ID_COLUMN
                                                    as i32))
            .and_then(value_to_playlist_id)
        {
            Some(id) => id,
            None => return,
        };
        let playlist_ref = match playlist::get_playlist_by_id(id) {
            Some(playlist) => playlist,
            None => {
                warn!("Tried to activate playlist ID {} by path {}, but it \
                       doesn't exist!", id, wo);
                return
            },
        };
        if Some(&playlist_ref) == self.active_playlist.as_ref() {
            return
        }
        self.playlist_edit_controller.as_ref().unwrap().borrow_mut()
            .set_selected_songs(&[]);
        self.active_playlist = Some(playlist_ref.clone());
        let _ =
            self.playlist_edit_controller.as_ref().unwrap().try_borrow_mut()
            .map(|mut x| x.activate_playlist(self.active_playlist.as_ref()
                                             .cloned()));
        self.playlist_generation.destroy();
        let playlist = playlist_ref.read().unwrap();
        drop(playlist);
        self.rebuild_playlist_view();
        let selection = self.playlists_view.get_selection();
        if selection.count_selected_rows() == 0 {
            selection.select_path(wo);
        }
    }
    fn periodic(&mut self, forced: bool) {
        self.update_view();
        self.update_scan_status();
        self.update_errors();
        self.maybe_update_playlist();
        if self.volume_changed {
            // TODO: do prefs updates in the background?
            match prefs::write() {
                Ok(_) => (),
                Err(x) => {
                    error!("While writing preferences: {:?}", x);
                },
            }
        }
        let timeout_ms =
            if forced || playback::get_playback_status().is_playing() { 100 }
            else { 1000 };
        let controller = match self.me.as_ref().and_then(Weak::upgrade) {
            None => return,
            Some(x) => x,
        };
        self.periodic_timer = Some(timeout_add_local(timeout_ms, move || {
            controller.borrow_mut().periodic(false);
            Continue(false)
        }));
    }
    fn force_periodic(&mut self) {
        match self.periodic_timer.take() {
            Some(x) => source_remove(x),
            None => (),
        }
        self.periodic(true);
    }
    fn force_periodic_soon(&mut self) {
        let controller = match self.me.as_ref().and_then(Weak::upgrade) {
            None => return,
            Some(x) => x,
        };
        match self.periodic_timer.take() {
            Some(x) => source_remove(x),
            None => (),
        }
        self.periodic_timer = Some(timeout_add_local(0, move || {
            controller.borrow_mut().periodic(false);
            Continue(false)
        }));
    }
    fn change_future_playlist(&mut self, neu: Option<PlaylistRef>) {
        match self.last_active_playlist.as_ref() {
            Some((_, x)) if Some(x) == neu.as_ref() => { return },
            Some((iter, _)) => {
                self.playlists_model.set_value(&iter, PLAYLIST_WEIGHT_COLUMN,
                                               &INACTIVE_WEIGHT.to_value());
            },
            None => (),
        }
        self.last_active_playlist = None;
        match neu.as_ref() {
            Some(neu_ref) => {
                // Do a linear search (ick!) for the correct row to hilight.
                let search_id = neu_ref.read().unwrap().get_id();
                let mut neu_iter = None;
                self.playlists_model.foreach(|model, _, iter| -> bool {
                    let found_id
                        = value_to_playlist_id(model.get_value
                                               (&iter,
                                                PLAYLIST_ID_COLUMN as i32));
                    if found_id == Some(search_id) {
                        model.downcast_ref::<TreeStore>().unwrap()
                            .set_value(&iter, PLAYLIST_WEIGHT_COLUMN,
                                       &ACTIVE_WEIGHT.to_value());
                        neu_iter = Some(iter.clone());
                        true
                    }
                    else {
                        false
                    }
                });
                if let Some(neu_iter) = neu_iter {
                    self.last_active_playlist
                        = Some((neu_iter, neu_ref.clone()));
                }
            },
            None => (),
        }
        playback::set_future_playlist(neu);
    }
    fn update_view(&mut self) {
        let (status, active_song) = playback::get_status_and_active_song();
        if status.is_playing() {
            set_icon(&self.play_button, "tsong-pause");
        }
        else {
            set_icon(&self.play_button, "tsong-play");
        }
        let active_song = match active_song {
            None => {
                self.osd.set_label("");
                None
            },
            Some((song_ref, time)) => {
                let song = song_ref.read().unwrap();
                let metadata = song.get_metadata();
                if self.remote_time != time {
                    self.remote_time = time;
                    self.remote.as_ref().unwrap().set_play_pos(time);
                }
                self.osd.set_label
                    (&format!("{} - {}\n{} / {}",
                              metadata.get("title").map(String::as_str)
                              .unwrap_or("Unknown Title"),
                              metadata.get("artist").map(String::as_str)
                              .unwrap_or("Unknown Artist"),
                              pretty_duration(time.floor() as u32),
                              pretty_duration(song.get_duration())));
                drop(song);
                Some(song_ref)
            },
        };
        if self.last_active_song.as_ref().map(|x| &x.1)
        != active_song.as_ref() {
            let playlist_model = self.playlist_model.as_ref().unwrap();
            match self.last_active_song.as_ref() {
                Some((Some(iter), _)) => {
                    playlist_model.set_value(&iter, SONG_WEIGHT_COLUMN,
                                             &INACTIVE_WEIGHT.to_value());
                },
                _ => (),
            }
            self.last_active_song = active_song.as_ref().map(|x| {
                (None, x.clone())
            });
            match active_song.as_ref() {
                Some(neu_ref) => {
                    // Do a linear search (ick!) for the correct row to
                    // hilight.
                    let search_id = neu_ref.read().unwrap().get_id();
                    let mut neu_iter = None;
                    playlist_model.foreach(|model, _, iter| -> bool {
                        let found_id
                            = value_to_song_id(model.get_value
                                               (&iter, SONG_ID_COLUMN as i32));
                        if found_id == Some(search_id) {
                            model.downcast_ref::<ListStore>().unwrap()
                                .set_value(&iter, SONG_WEIGHT_COLUMN,
                                           &ACTIVE_WEIGHT.to_value());
                            neu_iter = Some(iter.clone());
                            true
                        }
                        else {
                            false
                        }
                    });
                    if let Some(neu_iter) = neu_iter {
                        match &mut self.last_active_song {
                            Some(x) => x.0 = Some(neu_iter),
                            _ => (),
                        }
                    }
                },
                None => (),
            }
            // TODO: also do this if we edit the song's metadata while it's
            // playing
            self.remote.as_ref().unwrap().set_now_playing(active_song.as_ref());
        }
    }
    fn force_spinner_start(&self) {
        self.scan_spinner.start();
    }
    fn background_task_in_progress(&mut self) -> bool {
        let scan_in_progress = match self.scan_thread.get_result_nonblocking(){
            Err(x) => {
                // TODO: display this error
                error!("Scan thread crashed! {:?}", x);
                false
            },
            Ok((x, None)) => !x,
            Ok((true, _)) => unreachable!(),
            Ok((false, Some(Ok(_)))) => {
                // (We would try updating the playlist here, except that that
                // will already have happened, because `update_view()` is
                // called before us)
                true
            },
            Ok((false, Some(Err(x)))) => {
                // TODO: display this error
                error!("Error during scan! {:?}", x);
                true
            },
        };
        scan_in_progress
            || self.playlist_edit_controller.as_ref().unwrap().borrow()
            .script_is_in_progress()
    }
    fn update_scan_status(&mut self) {
        if self.background_task_in_progress() {
            self.scan_spinner.start();
        }
        else {
            self.scan_spinner.stop();
        }
    }
    fn update_errors(&mut self) -> Option<()> {
        if let Some((new_generation, errors)) = errors::if_newer_than(&self.errors_generation) {
            self.errors_generation = new_generation;
            if errors.is_empty() {
                self.errors_button.set_visible(false);
                self.errors_button.set_sensitive(false);
            }
            else {
                self.errors_button.set_visible(true);
                self.errors_button.set_sensitive(true);
                let mut total_error_count = 0;
                for (_, errors) in errors.iter() {
                    total_error_count += errors.len();
                }
                // TODO: i18n, plurality
                self.errors_button.set_tooltip_text
                    (Some(&format!("Errors: {}\nClick for more information.",
                                   total_error_count)));
            }
            self.errors_controller.as_ref().unwrap().try_borrow_mut().ok()?
                .update_if_visible(new_generation, errors);
        }
        None
    }
    fn maybe_update_playlist(&mut self) {
        let playlist_ref = match self.active_playlist.as_ref() {
            Some(x) => x,
            None => return,
        };
        let playlist = match playlist_ref.sheepishly_maybe_refreshed() {
            Some(x) => x,
            _ => return,
        };
        if playlist.get_playlist_generation() == self.playlist_generation {
            drop(playlist);
            // okay, but maybe some songs in it got changed?
            let changed_songs: Vec<SongID>
                = self.song_meta_update_rx.try_iter().collect();
            if changed_songs.is_empty() {
                return
            }
            else {
                // Upgrade to a write lock, because we might want to resort the
                // playlist and we won't want to repeat our checks.
                let mut playlist = playlist_ref.write().unwrap();
                if playlist.get_playlist_generation()
                == self.playlist_generation {
                    let changed_song_set: HashSet<SongID>
                        = changed_songs.iter().map(|x| *x).collect();
                    let songs_in_playlist = playlist.get_songs();
                    let mut changed_songs_in_playlist: HashSet<SongID>
                        = HashSet::with_capacity(songs_in_playlist.len()
                                                 .min(changed_songs.len()));
                    for song_ref in songs_in_playlist {
                        let song_id = song_ref.read().unwrap().get_id();
                        if changed_song_set.contains(&song_id) {
                            changed_songs_in_playlist.insert(song_id);
                        }
                    }
                    if changed_songs_in_playlist.len() == 0 {
                        // nope! nothing to do
                        return
                    }
                    // Okay, so at this point we know that the set of songs
                    // that are in the playlist hasn't changed. But maybe, if
                    // it's not shuffled, some metadata has changed that
                    // affected the sort?
                    let playlist_changed = if playlist.is_shuffled() { false }
                    else { playlist.resort(false) };
                    if !playlist_changed {
                        // The sort didn't change, but some of the songs at
                        // least did. Update their metadata in-place.
                        drop(playlist);
                        let playlist = playlist_ref.read().unwrap();
                        // ...after one last check that the playlist hasn't
                        // been updated out from under us.
                        if playlist.get_playlist_generation()
                        == self.playlist_generation {
                            // ...and making sure the in-place update goes
                            // smoothly...
                            match self.update_playlist_view
                                (playlist, changed_songs_in_playlist) {
                                    Ok(_) => return,
                                    Err(x) => error!("While doing in-place \
                                                      metadata update: {}", x),
                                }
                        }
                    }
                }
            }
        }
        else {
            drop(playlist);
        }
        self.rebuild_playlist_view();
    }
    fn clicked_play(&mut self) {
        let status = playback::get_playback_status();
        if status.is_playing() {
            playback::send_command(PlaybackCommand::Pause);
            set_icon(&self.play_button, "tsong-play");
        }
        else {
            let song_to_play = if status == PlaybackStatus::Stopped {
                self.change_future_playlist(self.active_playlist.clone());
                let playlist_model = self.playlist_model.as_ref().unwrap();
                self.playlist_view.get_cursor().0
                    .and_then(|x| playlist_model.get_iter(&x))
                    .map(|x| playlist_model.get_value(&x,
                                                      SONG_ID_COLUMN as i32))
                    .and_then(value_to_song_id)
                    .and_then(logical::get_song_by_song_id)
            } else { None };
            playback::send_command(PlaybackCommand::Play(song_to_play));
            set_icon(&self.play_button, "tsong-pause");
            self.force_periodic();
        }
    }
    fn edited_playlist_name_in_view(&self, wo: TreePath,
                                    nu: &str) -> Option<()> {
        let iter = self.playlists_model.get_iter(&wo)?;
        let value = self.playlists_model.get_value(&iter,
                                                   PLAYLIST_ID_COLUMN as i32);
        let playlist = value_to_playlist_id(value)
            .and_then(playlist::get_playlist_by_id)?;
        self.playlists_model.set_value(&iter, PLAYLIST_NAME_COLUMN,
                                       &Value::from(nu));
        playlist.write().unwrap().set_name(nu.to_owned());
        None
    }
    fn playlists_cursor_changed(&mut self) -> Option<()> {
        let wo = self.playlists_view.get_cursor().0?;
        self.activate_playlist_by_path(&wo);
        None
    }
    fn playlist_row_activated(&mut self, wo: &TreePath) -> Option<()> {
        let playlist_model = self.playlist_model.as_ref()?;
        let song = playlist_model.get_iter(wo)
            .map(|x| playlist_model.get_value(&x, PLAYLIST_ID_COLUMN as i32))
            .and_then(value_to_song_id)
            .and_then(logical::get_song_by_song_id);
        if let Some(song) = song {
            self.change_future_playlist(self.active_playlist.clone());
            playback::send_command(PlaybackCommand::Play(Some(song)));
            self.force_periodic();
        }
        None
    }
    fn clicked_shuffle(&mut self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        let now_active = playlist.write().unwrap().toggle_shuffle();
        self.shuffle_button.set_active(now_active);
        let _ = self.remote.as_ref().unwrap().set_is_shuffled(now_active);
        self.rebuild_playlist_view();
        None
    }
    fn clicked_playmode(&mut self) -> Option<()> {
        let playlist = self.active_playlist.as_ref()?;
        playlist.write().unwrap().bump_playmode();
        self.update_playmode_button();
        None
    }
    fn update_playmode_button(&mut self) -> Option<()> {
        match self.active_playlist.as_ref() {
            None => {
                self.playmode_button.set_sensitive(false);
                self.playmode_button.set_active(false);
                set_icon(&self.playmode_button, "tsong-loop");
                self.remote.as_ref().unwrap().set_cur_playmode(Playmode::End.into());
            },
            Some(playlist) => {
                self.playmode_button.set_sensitive(true);
                let playmode = playlist.read().unwrap().get_playmode();
                if playmode == Playmode::LoopOne {
                    set_icon(&self.playmode_button, "tsong-loop-one");
                }
                else {
                    set_icon(&self.playmode_button, "tsong-loop");
                }
                self.playmode_button.set_active(playmode != Playmode::End);
                self.remote.as_ref().unwrap().set_cur_playmode(playmode.into());
            }
        }
        None
    }
    fn clicked_new_playlist(&mut self) -> Option<()> {
        let selection = self.playlists_view.get_selection();
        let (mut wo_list, model) = selection.get_selected_rows();
        if wo_list.len() < 2 { wo_list.clear() }
        let child_row_ref_list: Vec<TreeRowReference>
            = wo_list.into_iter()
            .filter_map(|x| TreeRowReference::new(&model, &x))
            .collect();
        let child_list: Vec<PlaylistRef>
            = child_row_ref_list.iter()
            .filter_map(|x| x.get_path()
                        .and_then(|x| model.get_iter(&x))
                        .map(|x| model.get_value
                             (&x, PLAYLIST_ID_COLUMN as i32))
                        .and_then(value_to_playlist_id)
                        .and_then(playlist::get_playlist_by_id))
            .collect();
        assert_eq!(child_row_ref_list.len(), child_list.len());
        let playlist_ref = match playlist::create_new_playlist() {
            Ok(x) => x,
            Err(x) => {
                error!("Unable to create playlist: {:?}", x);
                return None
            }
        };
        let id = playlist_ref.read().unwrap().get_id();
        let mut expanded_playlist_ids = self.get_expanded_playlists();
        expanded_playlist_ids.push(id);
        for child_ref in child_list.iter() {
            child_ref.move_next_to(Some(&playlist_ref), None);
        }
        for child_row_ref in child_row_ref_list.into_iter() {
            if let Some(child_wo) = child_row_ref.get_path() {
                let child_iter = model.get_iter(&child_wo).unwrap();
                self.playlists_model.remove(&child_iter);
            }
        }
        let mut our_new_path = Vec::with_capacity(1);
        add_playlists_to_model(&self.playlists_model,
                               &[playlist_ref.clone()],
                               &mut our_new_path,
                               None, &[playlist_ref],
                               playback::get_future_playlist().as_ref());
        self.expand_playlists(expanded_playlist_ids);
        let iter = our_new_path.get(0).and_then(|x| model.get_iter(&x));
        match iter.and_then(|x| self.playlists_model.get_path(&x)) {
            Some(path) => {
                self.activate_playlist_by_path(&path);
                self.playlists_view
                    .set_cursor_on_cell(&path,
                                        Some(&self.playlist_name_column),
                                        Some(&self.playlist_name_cell),
                                        true);
            },
            _ => (),
        }
        None
    }
    fn delete_playlist_button_should_be_sensitive(&self) -> bool {
        // TODO: true if there is at least one top level playlist not selected
        playlist::get_top_level_playlists().len() > 1
    }
    fn clicked_delete_playlist(&mut self) -> Option<()> {
        if !self.delete_playlist_button_should_be_sensitive() { return None }
        let selection = self.playlists_view.get_selection();
        let (wo_list, model) = selection.get_selected_rows();
        let wo_list: Vec<TreeRowReference>
            = wo_list.into_iter()
            .filter_map(|x| TreeRowReference::new(&model, &x))
            .collect();
        for wo in wo_list.iter() {
            let playlist = match wo.get_path()
                .and_then(|x| self.playlists_model.get_iter(&x))
                .map(|x| self.playlists_model.get_value(&x, PLAYLIST_ID_COLUMN
                                                        as i32))
                .and_then(value_to_playlist_id)
                .and_then(playlist::get_playlist_by_id)
            {
                Some(x) => x,
                None => continue,
            };
            if Some(&playlist) == self.active_playlist.as_ref() {
                self.active_playlist = None;
            }
            playlist::delete_playlist(playlist);
        }
        let expanded_playlist_ids = self.get_expanded_playlists();
        let (neu_model, _, neu_active_playlist) = build_playlists_model(&[]);
        self.playlists_model = neu_model;
        self.playlists_view.set_model(Some(&self.playlists_model));
        self.expand_playlists(expanded_playlist_ids);
        self.last_active_playlist = neu_active_playlist;
        if self.active_playlist.is_none() {
            self.activate_playlist_by_path(&TreePath::new_first());
        }
        self.delete_playlist_button
            .set_sensitive(self.delete_playlist_button_should_be_sensitive());
        None
    }
    fn clicked_rollup(&mut self) {
        let mut geom = Geometry {
            min_width: -1, max_width: i32::MAX,
            min_height: -1, max_height: i32::MAX,
            base_width: -1, base_height: -1,
            width_inc: -1, height_inc: -1,
            min_aspect: -1.0, max_aspect: -1.0,
            win_gravity: Gravity::NorthWest,
        };
        let geom_mask = WindowHints::MAX_SIZE;
        if self.rollup_grid.get_visible() {
            geom.max_height = self.control_box.get_allocated_height();
            self.rolled_down_height = self.window.get_allocated_height();
            self.rollup_grid.hide();
            set_icon(&self.rollup_button, "tsong-rolldown");
            self.window.set_geometry_hints(Some(&self.window),
                                           Some(&geom), geom_mask);
        }
        else {
            self.rollup_grid.show();
            set_icon(&self.rollup_button, "tsong-rollup");
            self.window.set_geometry_hints(Some(&self.window),
                                           Some(&geom), geom_mask);
            self.window.resize(self.window.get_allocated_width(),
                               self.rolled_down_height);
        }
    }
    fn main_window_resized(&mut self, allocation: &Allocation) {
        if !self.rollup_grid.is_visible() {
            if allocation.height >= self.control_box.get_allocated_height()*3/2
            {
                self.clicked_rollup();
                self.rollup_button.set_sensitive(false);
            }
        }
    }
    fn clicked_settings(&mut self) -> Option<()> {
        if self.settings_button.get_active() {
            self.settings_controller.as_ref().unwrap().try_borrow_mut().ok()?
                .show();
        }
        else {
            self.settings_controller.as_ref().unwrap().try_borrow_mut().ok()?
                .unshow();
        }
        None
    }
    fn closed_settings(&mut self) {
        self.settings_button.set_active(false);
    }
    fn clicked_errors(&mut self) -> Option<()> {
        if self.errors_button.get_active() {
            self.errors_controller.as_ref().unwrap().try_borrow_mut().ok()?
                .show();
        }
        else {
            self.errors_controller.as_ref().unwrap().try_borrow_mut().ok()?
                .unshow();
        }
        None
    }
    fn closed_errors(&mut self) {
        self.errors_button.set_active(false);
    }
    fn clicked_playlist_edit(&mut self) -> Option<()> {
        if self.playlist_edit_button.get_active() {
            self.playlist_edit_controller.as_ref().unwrap().try_borrow_mut()
                .ok()?.show();
        }
        else {
            self.playlist_edit_controller.as_ref().unwrap().try_borrow_mut()
                .ok()?.unshow();
        }
        None
    }
    fn closed_playlist_edit(&mut self) {
        self.playlist_edit_button.set_active(false);
    }
    fn rescan(&mut self) {
        match self.scan_thread.rescan(prefs::get_music_paths()) {
            Ok(_) => (),
            Err(x) => warn!("Couldn't start music scan! {:?}", x),
        }
        self.force_periodic();
    }
    fn update_volume(&mut self, nu: f64) {
        prefs::set_volume(nu.floor() as i32);
        if nu > 0.0 {
            playback::set_mute(false);
        }
        self.volume_changed = true;
    }
    fn update_selected_songs(&self) {
        let selection = self.playlist_view.get_selection();
        let (selected_rows, model) = selection.get_selected_rows();
        let selected_songs: Vec<SongID> =
            selected_rows.into_iter()
            .filter_map(|path| model.get_iter(&path))
            .map(|iter| model.get_value(&iter, SONG_ID_COLUMN as i32))
            .filter_map(value_to_song_id)
            .collect();
        self.playlist_edit_controller.as_ref().unwrap().borrow_mut()
            .set_selected_songs(&selected_songs[..]);
    }
    fn edit_playlist(&mut self, neu_code: String,
                     neu_columns: Vec<playlist::Column>) {
        self.active_playlist.as_ref()
            .map(|x| x.write().unwrap()
                 .set_rule_code_and_columns(neu_code, neu_columns));
    }
    fn update_playlist_view(&self, playlist: RwLockReadGuard<Playlist>,
                            mut changed_songs: HashSet<SongID>)
    -> anyhow::Result<()> {
        let playlist_model = self.playlist_model.as_ref().unwrap();
        let mut did_ok = true;
        let manual_songs: HashSet<SongID>
            = playlist.get_manual_songs().iter().map(|x| *x).collect();
        playlist_model.foreach(|model, _wo, iter| {
            let id = value_to_song_id(model.get_value
                                      (iter, SONG_ID_COLUMN as i32)).unwrap();
            if changed_songs.contains(&id) {
                changed_songs.remove(&id);
                if let Some(song_ref) = logical::get_song_by_song_id(id) {
                    self.emplace_metadata(playlist_model, iter,
                                          playlist.get_columns(),
                                          &manual_songs,
                                          &song_ref.read().unwrap());
                }
                else {
                    did_ok = false;
                    return true
                }
            }
            changed_songs.is_empty()
        });
        match did_ok {
            true => {
                self.update_selected_songs();
                Ok(())
            },
            false => Err(anyhow!("A song got deleted out from under us?")),
        }
    }
    fn emplace_metadata(&self, playlist_model: &ListStore, iter: &TreeIter,
                        columns: &[playlist::Column],
                        manual_songs: &HashSet<SongID>,
                        song: &LogicalSong) {
        let metadata = song.get_metadata();
        playlist_model.set_value(&iter, SONG_IS_MANUAL_COLUMN,
                                 &manual_songs.contains(&song.get_id())
                                 .to_value());
        let mut column_index: u32 = SONG_FIRST_META_COLUMN;
        for column in columns {
            let s = if column.tag == "duration" {
                pretty_duration(song.get_duration()).to_value()
            }
            else {
                metadata.get(&column.tag).map(String::as_str)
                    .and_then(|x| if x.len() == 0 { None } else { Some(x)})
                    .to_value()
            };
            playlist_model.set_value(&iter, column_index, &s);
            column_index += 1;
        }
    }
    fn get_expanded_playlists(&mut self) -> Vec<PlaylistID> {
        let mut ret = Vec::new();
        self.playlists_view.map_expanded_rows(|_, wo| {
            self.playlists_model.get_iter(wo)
                .map(|x| self.playlists_model.get_value(&x, PLAYLIST_ID_COLUMN
                                                        as i32))
                .and_then(value_to_playlist_id)
                .map(|x| ret.push(x));
        });
        ret
    }
    fn expand_playlists(&mut self, mut playlist_ids: Vec<PlaylistID>) {
        self.playlists_model.foreach(|playlists_model, wo, iter| {
            let id = match value_to_playlist_id(playlists_model.get_value
                                                (&iter, PLAYLIST_ID_COLUMN
                                                 as i32)) {
                Some(x) => x,
                None => return playlist_ids.len() == 0,
            };
            for i in 0 .. playlist_ids.len() {
                if playlist_ids[i] == id {
                    playlist_ids.remove(i);
                    self.playlists_view.expand_row(wo, false);
                    break
                }
            }
            playlist_ids.len() == 0
        });
    }
    fn dragged_playlists_in_playlist_list(&mut self,
                                          data: &SelectionData,
                                          wo: TreePath,
                                          pos: TreeViewDropPosition,
                                          _time: u32) -> (bool, bool) {
        assert_eq!(data.get_format(), 8);
        assert_eq!(data.get_length() % 8, 0);
        let data = data.get_data();
        let playlists: Vec<PlaylistRef> = data.chunks_exact(8)
            .map(|x| PlaylistID::from_inner(u64::from_le_bytes(x.try_into()
                                                               .unwrap())))
            .filter_map(playlist::get_playlist_by_id)
            .collect();
        // TODO: make sure none of the selected playlists is a parent of the
        // drop destination, again
        let target_playlist_ref = match
            self.playlists_model.get_iter(&wo)
            .map(|x| self.playlists_model.get_value(&x, PLAYLIST_ID_COLUMN
                                                    as i32))
            .and_then(value_to_playlist_id)
            .and_then(playlist::get_playlist_by_id) {
                Some(x) => x,
                None => {
                    return (false, false)
                },
            };
        use TreeViewDropPosition::*;
        let (parent_ref, sibling_ref) = match pos {
            IntoOrBefore | IntoOrAfter => {
                (Some(target_playlist_ref), None)
            },
            Before => {
                let target_playlist = target_playlist_ref.as_ref().read().unwrap();
                let parent_ref = target_playlist.get_parent();
                drop(target_playlist);
                (parent_ref, Some(target_playlist_ref))
            },
            After => {
                let target_playlist = target_playlist_ref.as_ref().read().unwrap();
                let parent_ref = target_playlist.get_parent();
                drop(target_playlist);
                // not sure how to express this borrow safely, so let's clone
                let siblings
                    = parent_ref.as_ref().map(|x| x.read().unwrap()
                                              .get_children().to_owned())
                    .unwrap_or_else(|| playlist::get_top_level_playlists().clone());
                let mut sibling_ref = None;
                if siblings.len() > 1 {
                    for n in 0 .. siblings.len() - 1 {
                        if siblings[n] == target_playlist_ref {
                            sibling_ref = Some(siblings[n+1].clone());
                            break;
                        }
                    }
                }
                (parent_ref, sibling_ref)
            },
            _ => return (false, false)
        };
        for playlist_ref in playlists.iter() {
            playlist_ref.move_next_to(parent_ref.as_ref(), sibling_ref.as_ref());
        }
        let expanded_playlist_ids = self.get_expanded_playlists();
        let (neu_model, selected, _) = build_playlists_model(&playlists[..]);
        self.playlists_model = neu_model;
        self.playlists_view.set_model(Some(&self.playlists_model));
        self.expand_playlists(expanded_playlist_ids);
        for wo in selected.iter() {
            self.playlists_view.get_selection().select_path(&wo);
        }
        // TODO: select the moved playlists
        (true, false)
    }
    fn dragged_songs_onto_playlist_list(&mut self, data: &SelectionData,
                                        wo: TreePath,
                                        pos: TreeViewDropPosition,
                                        _time: u32) -> (bool, bool) {
        if pos != TreeViewDropPosition::IntoOrBefore
            && pos != TreeViewDropPosition::IntoOrAfter {return (false,false)}
        assert_eq!(data.get_format(), 8);
        assert_eq!(data.get_length() % 8, 0);
        let playlist_ref = match self.playlists_model.get_iter(&wo)
            .map(|x| self.playlists_model.get_value(&x, PLAYLIST_ID_COLUMN
                                                    as i32))
            .and_then(value_to_playlist_id)
            .and_then(playlist::get_playlist_by_id) {
                Some(x) => x,
                None => {
                    return (false, false)
                },
            };
        let data = data.get_data();
        let song_ids: Vec<SongID> = data.chunks_exact(8)
            .map(|x| SongID::from_inner(u64::from_le_bytes(x.try_into()
                                                           .unwrap())))
            .collect();
        let mut songs_right = &song_ids[..];
        let mut playlist = playlist_ref.write().unwrap();
        let mut songs_left = playlist.get_manual_songs();
        let mut new_songs = Vec::with_capacity
            (songs_left.len() + songs_right.len());
        // Merge the new songs onto the existing list, preserving sort and
        // uniqueness
        while !songs_left.is_empty() && !songs_right.is_empty() {
            match songs_left[0].cmp(&songs_right[0]) {
                Ordering::Less => {
                    new_songs.push(songs_left[0]);
                    songs_left = &songs_left[1..];
                },
                Ordering::Equal => {
                    new_songs.push(songs_left[0]);
                    songs_left = &songs_left[1..];
                    songs_right = &songs_right[1..];
                },
                Ordering::Greater => {
                    new_songs.push(songs_right[0]);
                    songs_right = &songs_right[1..];
                },
            }
        }
        assert!(songs_left.is_empty() || songs_right.is_empty());
        new_songs.extend_from_slice(songs_left);
        new_songs.extend_from_slice(songs_right);
        playlist.set_manual_songs(new_songs);
        drop(playlist);
        if Some(playlist_ref) == self.active_playlist {
            self.rebuild_playlist_view();
        }
        (true, false)
    }
    fn delete_selected_songs(&mut self) -> Option<()> {
        let active_playlist_ref = self.active_playlist.as_ref()?;
        let mut active_playlist = active_playlist_ref.write().unwrap();
        let selection = self.playlist_view.get_selection();
        let playlist_model = self.playlist_model.as_ref().unwrap();
        let (wo_list, _) = selection.get_selected_rows();
        let mut new_manual_list = None;
        for wo in wo_list.iter() {
            let iter = match playlist_model.get_iter(wo) {
                Some(x) => x,
                None => continue,
            };
            let is_manual: bool = playlist_model.get_value
                (&iter, SONG_IS_MANUAL_COLUMN as i32).get().unwrap()
                .unwrap_or(false);
            if is_manual {
                playlist_model.set_value(&iter, SONG_IS_MANUAL_COLUMN,
                                         &false.to_value());
                if new_manual_list.is_none() {
                    new_manual_list = Some(active_playlist.get_manual_songs()
                                           .to_vec());
                }
                let id = match value_to_song_id
                    (playlist_model.get_value(&iter, SONG_ID_COLUMN as i32)) {
                        Some(x) => x,
                        None => continue,
                    };
                let new_manual_list = new_manual_list.as_mut().unwrap();
                match new_manual_list.iter().position(|x| x == &id) {
                    None => (),
                    Some(index) => { new_manual_list.remove(index); },
                }
            }
        }
        match new_manual_list {
            None => (), // there were no manual songs
            Some(new_manual_list) => {
                active_playlist.set_manual_songs(new_manual_list);
                drop(active_playlist);
                self.rebuild_playlist_view();
            },
        }
        None
    }
    fn delete_manual_song(&mut self, path: &str) -> Option<()> {
        let wo = TreePath::from_string(path);
        let playlist_model = self.playlist_model.as_ref().unwrap();
        let iter = playlist_model.get_iter(&wo)?;
        let song_id = value_to_song_id
            (playlist_model.get_value(&iter, SONG_ID_COLUMN as i32))?;
        playlist_model.set_value(&iter, SONG_IS_MANUAL_COLUMN,
                                 &false.to_value());
        let active_playlist_ref = self.active_playlist.as_ref()?;
        let mut active_playlist = active_playlist_ref.write().unwrap();
        let old_manual_list = active_playlist.get_manual_songs();
        let new_manual_list = old_manual_list.iter()
            .filter_map(|x| if *x == song_id { None } else { Some(*x) })
            .collect();
        if new_manual_list == old_manual_list { return None }
        active_playlist.set_manual_songs(new_manual_list);
        drop(active_playlist);
        // We can't do `rebuild_playlist_view` in the signal handler directly,
        // or GTK+ starts throwing assertion failures. So let's set a 0ms timer
        // to do it instead.
        self.force_periodic_soon();
        None
    }
    fn update_volume_slider(&mut self) {
        set_volume_label(&self.volume_scale, &self.volume_label)
    }
}

impl RemoteTarget for Controller {
    fn remote_quit(&mut self) -> Option<()> {
        self.window.close();
        None
    }
    fn remote_raise(&mut self) -> Option<()> {
        self.window.present();
        None
    }
    fn remote_playpause(&mut self) -> Option<()> {
        self.clicked_play();
        None
    }
    fn remote_left(&mut self) -> Option<()> {
        // TODO: RTL
        self.remote_prev()
    }
    fn remote_right(&mut self) -> Option<()> {
        // TODO: RTL
        self.remote_next()
    }
    fn remote_prev(&mut self) -> Option<()> {
        playback::send_command(PlaybackCommand::Prev);
        None
    }
    fn remote_next(&mut self) -> Option<()> {
        playback::send_command(PlaybackCommand::Next);
        None
    }
    fn remote_quieten(&mut self) -> Option<()> {
        let cur_volume = prefs::get_volume();
        let nu_volume = (cur_volume - 5).max(prefs::MIN_VOLUME);
        if cur_volume == nu_volume { return None }
        self.volume_scale.set_value(nu_volume as f64);
        prefs::set_volume(nu_volume);
        if nu_volume > 0 {
            playback::set_mute(false);
        }
        self.volume_changed = true;
        None
    }
    fn remote_louden(&mut self) -> Option<()> {
        let cur_volume = prefs::get_volume();
        let nu_volume = (cur_volume + 5).max(prefs::MAX_VOLUME);
        if cur_volume == nu_volume { return None }
        self.volume_scale.set_value(nu_volume as f64);
        prefs::set_volume(nu_volume);
        playback::set_mute(false);
        self.volume_changed = true;
        None
    }
    fn remote_mute(&mut self) -> Option<()> {
        if playback::toggle_mute() {
            // we are now muted
            self.volume_scale.set_value(0.0);
        }
        else {
            // we are no longer muted
            self.volume_scale.set_value(prefs::get_volume() as f64);
        }
        // note: we don't actually set the playback volume here, but if the
        // user manipulates the slider, everything should work out how they
        // expect
        None
    }
    fn remote_set_volume(&mut self, nu: f64) -> Option<()> {
        let nu = (nu.max(0.0).min(2.0) * 100.0 + 0.5).floor() as i32;
        self.volume_scale.set_value(nu as f64);
        prefs::set_volume(nu);
        playback::set_mute(false);
        self.volume_changed = true;
        None
    }
    fn remote_set_shuffle(&mut self, shuffle: bool) -> Option<()> {
        let playlist_ref = self.active_playlist.as_ref()?;
        let mut playlist = playlist_ref.write().unwrap();
        if playlist.is_shuffled() != shuffle {
            self.shuffle_button.set_active(shuffle);
            playlist.set_shuffle(shuffle);
            drop(playlist);
            // this will result in borrowing the MprisPlayer again. let
            // periodic handle this.
            //self.rebuild_playlist_view();
        }
        None
    }
    fn remote_set_playmode(&mut self, nu: Playmode) -> Option<()> {
        let playlist_ref = self.active_playlist.as_ref()?;
        let mut playlist = playlist_ref.write().unwrap();
        if playlist.get_playmode() != nu {
            playlist.set_playmode(nu);
            drop(playlist);
            self.update_playmode_button();
        }
        None
    }
    fn remote_pause(&mut self) -> Option<()> {
        playback::send_command(PlaybackCommand::Pause);
        None
    }
    fn remote_play(&mut self) -> Option<()> {
        let status = playback::get_playback_status();
        if status.is_playing() {
            // unlike when PlayPause is clicked, do nothing
        }
        else {
            let song_to_play = if status == PlaybackStatus::Stopped {
                self.change_future_playlist(self.active_playlist.clone());
                let playlist_model = self.playlist_model.as_ref().unwrap();
                self.playlist_view.get_cursor().0
                    .and_then(|x| playlist_model.get_iter(&x))
                    .map(|x| playlist_model.get_value(&x,
                                                      SONG_ID_COLUMN as i32))
                    .and_then(value_to_song_id)
                    .and_then(logical::get_song_by_song_id)
            } else { None };
            playback::send_command(PlaybackCommand::Play(song_to_play));
            set_icon(&self.play_button, "tsong-pause");
            self.force_periodic();
        }
        None
    }
    fn remote_stop(&mut self) -> Option<()> {
        playback::send_command(PlaybackCommand::Stop);
        None
    }
    fn remote_shuffle(&mut self) -> Option<()> {
        self.clicked_shuffle();
        None
    }
    fn remote_playmode(&mut self) -> Option<()> {
        self.clicked_playmode();
        None
    }
}

fn add_klasoj<W>(widget: &W, klasoj: &[&str])
where W: IsA<Widget> {
    let style_context = widget.get_style_context();
    for klaso in klasoj {
        style_context.add_class(klaso);
    }
}

fn control_button_add<T, W>(control_box: &T, button: &W, klasoj: &[&str])
where T: IsA<Container>, W: IsA<Widget> {
    add_klasoj(button, klasoj);
    let nu_box = BoxBuilder::new().orientation(Orientation::Vertical)
        .valign(Align::Center).build();
    nu_box.pack_start(button, false, false, 0);
    control_box.add(&nu_box);
}

pub fn go() {
    let application = Application::new(
        Some("name.bizna.tsong"),
        Default::default(),
    ).expect("failed to initialize the GTK application (!?!)");
    application.connect_activate(|_|{});
    // Create our main window at startup, not at activate, so that multiple
    // instances of the main window don't end up appearing and fighting their
    // alternate-timeline counterparts Rick and Morty style.
    application.connect_startup(|application| {
        // Controller will hook itself in and keep track of its own lifetime
        let _ = Controller::new(application);
    });
    #[cfg(not(target_os = "windows"))]
    {
        let argv: Vec<String> = std::env::args().collect();
        application.run(&argv[..]);
    }
    // on Windows, don't parse a command line, because GTK+ just retrieves the
    // application's command line via system call anyway, so that the calling
    // app can't screw up Unicode handling... -_-
    #[cfg(target_os = "windows")]
    application.run(&[]);
}

fn pretty_duration(t: u32) -> String {
    if t >= 86400 {
        format!("{}:{:02}:{:02}:{:02}",
                t / 86400, (t / 3600) % 24, (t / 60) % 60, t % 60)
    }
    else if t >= 3600 {
        format!("{}:{:02}:{:02}",
                t / 3600, (t / 60) % 60, t % 60)
    }
    else {
        format!("{}:{:02}",
                t / 60, t % 60)
    }
}

/// Take a metadata tag name and return its human-readable name.
fn make_column_heading(orig: &str) -> String {
    let mut ret = Vec::with_capacity(orig.as_bytes().len());
    let mut spaced = true;
    for b in orig.as_bytes() {
        if *b == b'_' || *b == b' ' {
            if ret.len() > 0 {
                ret.push(b' ');
                spaced = true;
            }
        }
        else {
            if *b >= b'a' && *b <= b'z' && spaced {
                ret.push(b - 0x20);
            }
            else {
                ret.push(*b);
            }
            spaced = false;
        }
    }
    while ret.len() > 0 && ret[ret.len()-1] == b' ' {
        ret.pop();
    }
    // This unsafe code is safe because the original string was valid UTF-8,
    // and we only transformed it by removing ASCII space and underscore from
    // the beginning or end (safe) or transforming ASCII lowercase into ASCII
    // uppercase (safe) or transforming ASCII underscore into ASCII space
    // (safe).
    let ret = unsafe { String::from_utf8_unchecked(ret) };
    // TODO: pass the "Englishified" heading into internationalization when we
    // add that. This will not only make the above, highly-English-specific,
    // code functional, but will replace the following hack.
    if ret == "Duration" { "Time".to_owned() }
    else { ret }
}

const PLAYLIST_ID_TYPE: Type = Type::U64;
const SONG_ID_TYPE: Type = Type::U64;

fn playlist_id_to_value(id: PlaylistID) -> Value {
    id.as_inner().to_value()
}

fn value_to_playlist_id(id: Value) -> Option<PlaylistID> {
    id.get().ok().and_then(|x| x).map(PlaylistID::from_inner)
}

fn song_id_to_value(id: SongID) -> Value {
    id.as_inner().to_value()
}

fn value_to_song_id(id: Value) -> Option<SongID> {
    id.get().ok().and_then(|x| x).map(SongID::from_inner)
}

fn add_playlists_to_model(playlists_model: &TreeStore,
                          selected_playlists: &[PlaylistRef],
                          selection_paths: &mut Vec<TreePath>,
                          parent_iterator: Option<&TreeIter>,
                          children: &[PlaylistRef],
                          active_playlist: Option<&PlaylistRef>)
-> Option<(TreeIter,PlaylistRef)> {
    let mut ret = None;
    for playlist_ref in children.iter() {
        let playlist = playlist_ref.read().unwrap();
        let id = playlist.get_id();
        let weight: u32 =
            if Some(playlist_ref) == active_playlist { ACTIVE_WEIGHT }
            else { INACTIVE_WEIGHT };
        let iter
            = playlists_model.insert_with_values(parent_iterator, None,
                                                 &[0, 1, 2],
                                                 &[&playlist_id_to_value(id),
                                                   &playlist.get_name(),
                                                   &weight]);
        if selected_playlists.contains(playlist_ref) {
            match playlists_model.get_path(&iter) {
                Some(x) => selection_paths.push(x),
                None => (),
            }
        }
        if Some(playlist_ref) == active_playlist {
            ret = Some((iter.clone(), playlist_ref.clone()));
        }
        ret = ret.or(add_playlists_to_model(playlists_model,
                                            selected_playlists,
                                            selection_paths,
                                            Some(&iter),
                                            playlist.get_children(),
                                            active_playlist));
    }
    ret
}

/// Returns:
///
/// 1. The new `TreeStore` containing an up to date model of the playlists
/// 2. The new list of paths within the `TreeStore` of selected playlists
///    (excluding any playlists that weren't in the new model)
/// 3. The iterator to the currently active playlist, and a reference to it
fn build_playlists_model(selected_playlists: &[PlaylistRef])
-> (TreeStore, Vec<TreePath>, Option<(TreeIter,PlaylistRef)>) {
    let active_playlist = playback::get_future_playlist();
    let playlists_model = TreeStore::new(&[PLAYLIST_ID_TYPE,Type::String,
                                           Type::U32]);
    assert!(playlists_model.get_flags()
            .contains(TreeModelFlags::ITERS_PERSIST));
    let mut selection_paths = Vec::with_capacity(selected_playlists.len());
    let neu_active_playlist =
        add_playlists_to_model(&playlists_model, selected_playlists,
                               &mut selection_paths, None,
                               &playlist::get_top_level_playlists()[..],
                               active_playlist.as_ref());
    (playlists_model, selection_paths, neu_active_playlist)
}

/// Set the icon on a widget.
fn set_icon<B: IsA<Button>>(button: &B, icon: &'static str) {
    let button = button.upcast_ref();
    button.set_image(Some(&Image::from_icon_name(Some(icon),
                                                 IconSize::LargeToolbar)));
    button.set_label("");
    let _ = button.set_property("always-show-image", &true);
}

fn playlist_search_func(model: &TreeModel, _: i32, search_string: &str,
                        iter: &TreeIter) -> bool {
    let fuse = Fuse::default();
    let search_pattern = fuse.create_pattern(search_string);
    let num_columns = model.get_n_columns();
    for n in SONG_FIRST_META_COLUMN as i32 .. num_columns {
        let value: Option<String> = model.get_value(&iter, n).get().unwrap();
        let value = match value {
            Some(x) => x,
            None => continue,
        };
        match fuse.search(search_pattern.as_ref(), &value) {
            Some(result) if result.score < 0.2 => {
                return false
            },
            _ => (),
        }
    }
    true
}

fn check_drag_onto_playlist<'a>
    (playlists_view: &TreeView, context: &DragContext,
     x: i32, y: i32, _time: u32,
     tsong_songs_mimetype_atom: &'a Atom,
     tsong_playlists_mimetype_atom: &'a Atom)
-> Option<(TreePath, TreeViewDropPosition, DragAction, &'a Atom)> {
    let targets = context.list_targets();
    let target = match targets.get(0) {
        None => return None,
        Some(target) => target,
    };
    if target == tsong_songs_mimetype_atom {
        let res = playlists_view.get_dest_row_at_pos(x, y);
        match res {
            Some((Some(wo), _)) =>
                Some((wo, TreeViewDropPosition::IntoOrBefore,
                      DragAction::LINK, tsong_songs_mimetype_atom)),
            _ => None,
        }
    }
    else if target == tsong_playlists_mimetype_atom {
        let res = playlists_view.get_dest_row_at_pos(x, y);
        match res {
            Some((Some(wo), pos)) => {
                // make sure none of the selected rows is either the target row
                // or one of its parents
                let selection = playlists_view.get_selection();
                let (selected_rows, _model) = selection.get_selected_rows();
                let mut check_row = wo.clone();
                while check_row.get_depth() > 0 {
                    if selected_rows.contains(&check_row) {
                        return None
                    }
                    if !check_row.up() {
                        break;
                    }
                }
                Some((wo, pos,
                      DragAction::MOVE, tsong_playlists_mimetype_atom))
            },
            _ => None,
        }
    }
    // We don't have any other supported drag sources
    else {
        None
    }
}

fn set_volume_label(scale: &Scale, label: &Label) {
    let val = scale.get_value().floor().min(200.0).max(0.0) as i32;
    if val > 100 {
        label.get_style_context().add_class("overblood");
    }
    else {
        label.get_style_context().remove_class("overblood");
    }
    if val > 0 {
        if prefs::get_show_decibels_on_volume_slider() {
            let db = playback::volume_to_db(val);
            label.set_label(&format!("{}% ({:+.2}dB)", val, db))
        }
        else {
            label.set_label(&format!("{}%", val))
        }
    }
    else {
        label.set_label("Muted"); // TODO: i18n
    }
}
