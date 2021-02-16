use crate::*;
use gtk::{
    prelude::*,
    Align,
    BoxBuilder,
    ButtonBoxBuilder,
    Button, ButtonBuilder,
    CellRendererText,
    ComboBox, ComboBoxBuilder,
    LabelBuilder,
    ListStore,
    Orientation,
    SeparatorBuilder,
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
    apply_button: Button,
    cancel_button: Button,
    ok_button: Button,
    hostapi_view: ComboBox,
    hostapi_model: ListStore,
    audiodev_view: ComboBox,
    audiodev_model: ListStore,
}

impl Controller {
    pub fn new() -> Rc<RefCell<Controller>> {
        let pa = PortAudio::new().expect("Could not initialize PortAudio");
        let window = WindowBuilder::new()
            .name("settings").type_(WindowType::Toplevel)
            .title("Tsong - Settings").build();
        let big_view = BoxBuilder::new()
            .name("settings").spacing(4).orientation(Orientation::Vertical)
            .build();
        window.add(&big_view);
        big_view.add(&LabelBuilder::new()
                     .label("Audio API:").halign(Align::Start).build());
        let renderer = CellRendererText::new();
        let hostapi_view = ComboBoxBuilder::new()
            .name("hostapi_view").build();
        hostapi_view.pack_start(&renderer, true);
        hostapi_view.add_attribute(&renderer, "text", 1);
        big_view.add(&hostapi_view);
        big_view.add(&LabelBuilder::new()
                    .label("Audio Device:").halign(Align::Start).build());
        let audiodev_view = ComboBoxBuilder::new()
            .name("audiodev_view").build();
        audiodev_view.pack_start(&renderer, true);
        audiodev_view.add_attribute(&renderer, "text", 1);
        big_view.add(&audiodev_view);
        big_view.pack_start(&SeparatorBuilder::new()
                          .orientation(Orientation::Horizontal).build(),
                          true, true, 6);
        // The buttons!
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
        big_view.add(&buttons_box);
        let ret = Rc::new(RefCell::new(Controller {
            window,
            pa,
            hostapi_view,
            audiodev_view,
            apply_button,
            cancel_button,
            ok_button,
            hostapi_model: ListStore::new(&[Type::U32, Type::String]),
            audiodev_model: ListStore::new(&[Type::U32, Type::String]),
            me: None
        }));
        let mut this = ret.borrow_mut();
        this.me = Some(Rc::downgrade(&ret));
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
        drop(this);
        ret
    }
    fn changed_hostapi(&mut self) {
        self.populate_audiodev();
    }
    fn populate_hostapi(&mut self) {
        self.hostapi_model = ListStore::new(&[Type::U32, Type::String]);
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
        self.audiodev_model.get_value(&iter, 0).get::<u32>()
            .unwrap()
    }
    fn populate_audiodev(&mut self) {
        let selected_api_index = self.get_selected_api();
        let selected_api_info = self.pa.host_api_info(selected_api_index)
            .unwrap();
        self.audiodev_model = ListStore::new(&[Type::U32, Type::String]);
        let new_row = self.audiodev_model.append();
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
                        eprintln!("Error enumerating PortAudio devices! {:?}",
                                  x);
                        continue
                    },
                };
            let info = match self.pa.device_info(index) {
                Ok(x) => x,
                Err(x) => {
                    eprintln!("Error enumerating PortAudio devices! {:?}",
                              x);
                    continue
                },
            };
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
    fn clicked_apply(&mut self) {
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
        prefs::set_chosen_audio_api_and_device(&self.pa, api_index,
                                               api_info.name, dev);
    }
    fn clicked_cancel(&mut self) {
        self.window.hide();
    }
    fn clicked_ok(&mut self) {
        self.clicked_apply();
        match prefs::write() {
            Ok(_) => (),
            Err(x) => {
                // TODO: error dialog
                eprintln!("Error writing prefs: {:?}", x);
                return
            },
        }
        self.window.hide();
    }
    pub fn show(&mut self) {
        self.populate_hostapi();
        self.window.show_all();
    }
}
