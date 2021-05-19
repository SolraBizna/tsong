use crate::*;
use log::error;
use gtk::{
    prelude::*,
    Adjustment,
    Align,
    BoxBuilder,
    ButtonBoxBuilder, ButtonBoxStyle,
    Button, ButtonBuilder,
    CellRendererText,
    CheckButton,
    ComboBox, ComboBoxBuilder,
    FileChooserDialog, FileChooserAction,
    LabelBuilder,
    ListStore,
    Orientation,
    PolicyType,
    PositionType,
    ResponseType,
    Scale, ScaleBuilder,
    ScrolledWindowBuilder,
    SeparatorBuilder,
    TreeView, TreeViewBuilder, TreeViewColumn,
    Window, WindowBuilder, WindowType,
};
use glib::{
    Type
};
use std::{
    cell::RefCell,
    rc::{Rc,Weak},
};
use portaudio::{
    DeviceIndex,
    HostApiIndex,
    PortAudio,
};

pub struct Controller {
    window: Window,
    pa: PortAudio,
    me: Option<Weak<RefCell<Controller>>>,
    parent: Weak<RefCell<super::Controller>>,
    apply_button: Button,
    cancel_button: Button,
    ok_button: Button,
    delete_location_button: Button,
    new_location_button: Button,
    resample_audio_box: CheckButton,
    show_decibels_box: CheckButton,
    hostapi_view: ComboBox,
    hostapi_model: ListStore,
    audiodev_view: ComboBox,
    audiodev_model: ListStore,
    locations_view: TreeView,
    locations_model: ListStore,
    desired_latency_slider: Scale,
    decode_ahead_slider: Scale,
}

impl Controller {
    pub fn new(parent: Weak<RefCell<super::Controller>>)
    -> Rc<RefCell<Controller>> {
        let pa = PortAudio::new().expect("Could not initialize PortAudio");
        let window = WindowBuilder::new()
            .name("settings").type_(WindowType::Toplevel)
            .title("Tsong - Settings").build();
        let big_box = BoxBuilder::new()
            .name("settings").spacing(4).orientation(Orientation::Vertical)
            .build();
        window.add(&big_box);
        big_box.add(&LabelBuilder::new()
                    .label("Audio API:").halign(Align::Start).build());
        let renderer = CellRendererText::new();
        let hostapi_view = ComboBoxBuilder::new()
            .tooltip_text("Which audio API to use. (Advanced)")
            .name("hostapi_view").build();
        hostapi_view.pack_start(&renderer, true);
        hostapi_view.add_attribute(&renderer, "text", 1);
        big_box.add(&hostapi_view);
        big_box.add(&LabelBuilder::new()
                    .label("Audio Device:").halign(Align::Start).build());
        let audiodev_view = ComboBoxBuilder::new()
            .tooltip_text("Which output device to use.")
            .name("audiodev_view").build();
        audiodev_view.pack_start(&renderer, true);
        audiodev_view.add_attribute(&renderer, "text", 1);
        big_box.add(&audiodev_view);
        big_box.add(&LabelBuilder::new()
                    .label("Desired Latency: (seconds)")
                    .halign(Align::Start).build());
        let desired_latency_slider = ScaleBuilder::new()
            .has_origin(true)
            .draw_value(true)
            .value_pos(PositionType::Bottom)
            .tooltip_text("Amount of audio data to send to the sound card at \
                           a time. Larger values lead to lower CPU usage, but \
                           make the volume slider less responsive. (Advanced)")
            .build();
        desired_latency_slider.set_digits(2);
        big_box.add(&desired_latency_slider);
        big_box.add(&LabelBuilder::new()
                    .label("Decode Ahead: (seconds)")
                    .halign(Align::Start).build());
        let decode_ahead_slider = ScaleBuilder::new()
            .has_origin(true)
            .draw_value(true)
            .value_pos(PositionType::Bottom)
            .show_fill_level(true)
            .restrict_to_fill_level(false)
            .tooltip_text("Amount of song data to decode ahead of playback. \
                           This will always be at least thrice the desired \
                           latency.\n\n\
                           Larger values may lead to lower CPU usage during \
                           playback, but too large a value could lead to \
                           large spikes in CPU usage and strange behavior \
                           of the Next and Previous buttons.\n\n\
                           (Advanced)")
            .build();
        decode_ahead_slider.set_digits(1);
        big_box.add(&decode_ahead_slider);
        let decode_ahead_clone = decode_ahead_slider.clone();
        desired_latency_slider.connect_value_changed(move |slider| {
            let value = slider.get_value();
            decode_ahead_clone.set_fill_level(value * 3.0);
        });
        // A checkbox!
        let resample_audio_box = CheckButton::with_label
            ("Resample audio");
        resample_audio_box.set_tooltip_text
            (Some("If checked, we will resample all audio to the best native \
                   sample rate for the selected output device. If unchecked, \
                   we will let the OS handle that for us. (Advanced)"));
        big_box.add(&resample_audio_box);
        // Another checkbox!
        let show_decibels_box = CheckButton::with_label
            ("Show decibels on volume slider");
        big_box.add(&show_decibels_box);
        // The music paths!
        big_box.add(&LabelBuilder::new()
                     .label("Music Locations:").halign(Align::Start).build());
        let locations_window = ScrolledWindowBuilder::new()
            .name("locations")
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        let locations_view = TreeViewBuilder::new()
            .tooltip_text("List of locations on your filesystem that this \
                           copy of Tsong will scan for songs.\n\n\
                           Use the buttons below to add and remove elements.")
            .headers_visible(false).reorderable(true).build();
        let location_column = TreeViewColumn::new();
        let location_cell = CellRendererText::new();
        location_column.pack_start(&location_cell, true);
        location_column.add_attribute(&location_cell, "text", 0);
        locations_view.append_column(&location_column);
        locations_window.add(&locations_view);
        big_box.add(&locations_window);
        let location_button_box = ButtonBoxBuilder::new()
            .layout_style(ButtonBoxStyle::Expand)
            .build();
        let delete_location_button = ButtonBuilder::new()
            .tooltip_text("Delete the selected location from the list of \
                           locations to scan for songs.")
            .build();
        delete_location_button.set_sensitive(false);
        location_button_box.add(&delete_location_button);
        super::set_icon(&delete_location_button, "tsong-remove");
        let new_location_button = ButtonBuilder::new()
            .tooltip_text("Add a new location to the list of locations to \
                           scan for songs.")
            .build();
        location_button_box.add(&new_location_button);
        big_box.add(&location_button_box);
        super::set_icon(&new_location_button, "tsong-add");
        // The buttons!
        big_box.pack_start(&SeparatorBuilder::new()
                            .orientation(Orientation::Horizontal).build(),
                            false, true, 6);
        let buttons_box = BoxBuilder::new()
            .orientation(Orientation::Horizontal).build();
        let button_box = ButtonBoxBuilder::new()
            .spacing(6).build();
        let cancel_button = ButtonBuilder::new()
            .tooltip_text("Close this window without saving your changes.")
            .label("_Cancel").use_underline(true).build();
        buttons_box.pack_start(&cancel_button, false, true, 0);
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
            window,
            pa,
            parent,
            hostapi_view,
            audiodev_view,
            locations_model: ListStore::new(&[Type::String]),
            locations_view,
            apply_button,
            cancel_button,
            ok_button,
            delete_location_button,
            new_location_button,
            decode_ahead_slider, desired_latency_slider,
            resample_audio_box, show_decibels_box,
            hostapi_model: ListStore::new(&[Type::U32, Type::String]),
            audiodev_model: ListStore::new(&[Type::U32, Type::String]),
            me: None
        }));
        let mut this = ret.borrow_mut();
        this.me = Some(Rc::downgrade(&ret));
        let controller = ret.clone();
        this.window.connect_delete_event(move |window, _| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.cleanup());
            window.hide_on_delete()
        });
        let controller = ret.clone();
        this.hostapi_view.connect_property_active_notify(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.changed_hostapi());
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
        this.delete_location_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_delete_location());
        });
        let controller = ret.clone();
        this.new_location_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_new_location());
        });
        let delete_location_button = this.delete_location_button.clone();
        this.locations_view.connect_cursor_changed(move |locations_view| {
            // this doesn't reference Controller because we *want* it to update
            // automatically, even when we caused the change
            delete_location_button.set_sensitive
                (locations_view.get_cursor().0.is_some())
        });
        drop(this);
        ret
    }
    fn changed_hostapi(&mut self) {
        self.populate_audiodev();
    }
    fn populate_hostapi(&mut self) {
        self.hostapi_model.clear();
        let default_index = self.pa.default_host_api().unwrap();
        let selected_index = prefs::get_chosen_audio_api(&self.pa);
        let mut selected_iter = None;
        let mut num_choices = 0;
        for (index, info) in self.pa.host_apis() {
            if info.default_output_device.is_none() { continue }
            let new_row = self.hostapi_model.append();
            self.hostapi_model.set_value(&new_row, 0,
                                         &(index as u32).to_value());
            if index == default_index {
                // TODO: i18n
                self.hostapi_model.set_value(&new_row, 1,
                                             &format!("{} (default)",
                                                      info.name).to_value());
            }
            else {
                self.hostapi_model.set_value(&new_row, 1,
                                             &info.name.to_value());
            }
            if index == selected_index || selected_iter.is_none() {
                selected_iter = Some(new_row);
            }
            num_choices += 1;
        }
        self.hostapi_view.set_model(Some(&self.hostapi_model));
        self.hostapi_view.set_active_iter(selected_iter.as_ref());
        self.hostapi_view.set_sensitive(num_choices > 1);
        self.populate_audiodev();
    }
    fn get_selected_api(&mut self) -> HostApiIndex {
        let iter = self.hostapi_view.get_active_iter().unwrap();
        self.hostapi_model.get_value(&iter, 0).get::<u32>()
            .unwrap().unwrap() as HostApiIndex
    }
    fn get_selected_dev(&mut self) -> Option<u32> {
        let iter = self.audiodev_view.get_active_iter().unwrap();
        let ret = self.audiodev_model.get_value(&iter, 0).get::<u32>()
            .unwrap().unwrap();
        if ret == u32::MAX { None }
        else { Some(ret) }
    }
    fn populate_audiodev(&mut self) {
        let selected_api_index = self.get_selected_api();
        let selected_api_info = self.pa.host_api_info(selected_api_index)
            .unwrap();
        self.audiodev_model.clear();
        let new_row = self.audiodev_model.append();
        self.audiodev_model.set_value(&new_row, 0, &u32::MAX.to_value());
        self.audiodev_model.set_value(&new_row, 1,
                                      &"Default Device".to_value());
        let mut selected_iter = self.audiodev_model.get_iter_first();
        let chosen_dev = prefs::get_chosen_audio_device_for_api(&self.pa,
                                                           selected_api_index);
        for n in 0 .. selected_api_info.device_count {
            let index = match self.pa.api_device_index_to_device_index
                (selected_api_index, n as i32) {
                    Ok(x) => x,
                    Err(x) => {
                        error!("While enumerating PortAudio devices! {:?}", x);
                        continue
                    },
                };
            let info = match self.pa.device_info(index) {
                Ok(x) => x,
                Err(x) => {
                    error!("While enumerating PortAudio devices! {:?}", x);
                    continue
                },
            };
            if info.max_output_channels < 1 { continue }
            let new_row = self.audiodev_model.append();
            if Some(n) == chosen_dev {
                selected_iter = Some(new_row.clone());
            }
            let DeviceIndex(index) = index;
            self.audiodev_model.set_value(&new_row, 0,
                                          &n.to_value());
            if index == selected_api_info.default_output_device.unwrap().0
            as u32 {
                // TODO: i18n
                self.audiodev_model.set_value(&new_row, 1,
                                             &format!("{} (default)",
                                                      info.name).to_value());
            }
            else {
                self.audiodev_model.set_value(&new_row, 1,
                                             &info.name.to_value());
            }
        }
        self.audiodev_view.set_model(Some(&self.audiodev_model));
        self.audiodev_view.set_active_iter(selected_iter.as_ref());
    }
    fn populate_sliders(&mut self) -> Option<()> {
        let desired_latency = prefs::get_desired_latency();
        let latency_adjustment = Adjustment::new
            (desired_latency,
             prefs::MIN_DESIRED_LATENCY, prefs::MAX_DESIRED_LATENCY + 0.1,
             0.01, 0.1, 0.1);
        self.desired_latency_slider.set_adjustment(&latency_adjustment);
        let decode_adjustment = Adjustment::new
            (prefs::get_decode_ahead(),
             prefs::MIN_DECODE_AHEAD, prefs::MAX_DECODE_AHEAD + 0.1,
             0.1, 0.1, 0.1);
        self.decode_ahead_slider.set_adjustment(&decode_adjustment);
        self.decode_ahead_slider.set_fill_level(desired_latency * 3.0);
        None
    }
    fn populate_locations(&mut self) {
        let src = prefs::get_music_paths();
        self.locations_model.clear();
        for path in src.iter() {
            self.locations_model.insert_with_values(None, &[0], &[&path]);
        }
        self.locations_view.set_model(Some(&self.locations_model));
    }
    fn clicked_apply(&mut self) -> Option<()> {
        let api_index = self.get_selected_api();
        let dev_index = self.get_selected_dev();
        let api_info = self.pa.host_api_info(api_index)
            .unwrap();
        let dev = dev_index.map(|dev_index| {
            let global_dev_index = self.pa.api_device_index_to_device_index
                (api_index, dev_index as i32).unwrap();
            let dev_info = self.pa.device_info(global_dev_index).unwrap();
            (dev_index, dev_info.name)
        });
        let mut dirs = Vec::new();
        self.locations_model.foreach(|model, _path, iter| {
            let value = model.get_value(&iter, 0);
            match value.get() {
                Ok(Some(x)) => dirs.push(x),
                _ => (),
            }
            false
        });
        prefs::set_music_paths(dirs);
        // (we wrote this or-chain this way because we don't want a short
        // circuiting OR)
        let mut needs_restart = false;
        needs_restart =
            prefs::set_chosen_audio_api_and_device
            (&self.pa, api_index, api_info.name, dev)
            || needs_restart;
        needs_restart =
            prefs::set_desired_latency
            (self.desired_latency_slider.get_value())
            || needs_restart;
        needs_restart =
            prefs::set_decode_ahead(self.decode_ahead_slider.get_value())
            || needs_restart;
        needs_restart =
            prefs::set_show_decibels_on_volume_slider
            (self.show_decibels_box.get_active())
            || needs_restart;
        needs_restart =
            prefs::set_resample_audio(self.resample_audio_box.get_active())
            || needs_restart;
        if needs_restart {
            if playback::get_playback_status() == PlaybackStatus::Playing {
                // force playback to be restarted
                playback::send_command(PlaybackCommand::Pause);
                playback::send_command(PlaybackCommand::Play(None));
            }
        }
        let parent = self.parent.upgrade()?;
        let mut parent = parent.try_borrow_mut().ok()?;
        parent.update_volume_slider();
        parent.rescan();
        None
    }
    fn clicked_cancel(&mut self) {
        self.window.close();
        self.cleanup();
    }
    fn clicked_ok(&mut self) {
        self.clicked_apply();
        match prefs::write() {
            Ok(_) => (),
            Err(x) => {
                // TODO: error dialog
                error!("While writing prefs: {:?}", x);
                return
            },
        }
        self.window.close();
        self.cleanup();
    }
    fn clicked_delete_location(&mut self) -> Option<()> {
        let wo = self.locations_view.get_cursor().0?;
        self.locations_model.get_iter(&wo)
            .map(|x| self.locations_model.remove(&x));
        None
    }
    fn clicked_new_location(&mut self) -> Option<()> {
        let dialog = FileChooserDialog::with_buttons
            (Some("Choose Music Location"), Some(&self.window),
             FileChooserAction::SelectFolder,
             &[("_Cancel", ResponseType::Cancel),
               ("_Open", ResponseType::Accept)]);
        let response = dialog.run();
        dialog.close();
        if response != ResponseType::Accept { return None }
        let path = dialog.get_filename()?;
        let path = path.into_os_string();
        let path = match path.into_string() {
            Ok(x) => x,
            Err(_) => {
                // TODO: Error dialog
                error!("The selected path contains invalid Unicode \
                        characters. We don't support such paths.");
                return None
            },
        };
        self.locations_model.insert_with_values(None, &[0], &[&path]);
        None
    }
    fn cleanup(&mut self) -> Option<()> {
        self.locations_model.clear();
        self.audiodev_model.clear();
        self.hostapi_model.clear();
        let parent = self.parent.upgrade()?;
        parent.try_borrow_mut().ok()?.closed_settings();
        None
    }
    pub fn show(&mut self) {
        if !self.window.is_visible() {
            self.populate_hostapi();
            self.populate_locations();
            self.populate_sliders();
            self.show_decibels_box.set_active
                (prefs::get_show_decibels_on_volume_slider());
            self.resample_audio_box.set_active(prefs::get_resample_audio());
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
}
