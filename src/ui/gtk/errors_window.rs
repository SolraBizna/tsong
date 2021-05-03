use crate::*;
use gtk::{
    prelude::*,
    BoxBuilder,
    Button, ButtonBuilder,
    Orientation,
    ScrolledWindowBuilder,
    TextView, TextViewBuilder,
    Window, WindowBuilder, WindowType,
    WrapMode,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::{Rc, Weak},
    sync::RwLockReadGuard,
};

pub struct Controller {
    window: Window,
    me: Option<Weak<RefCell<Controller>>>,
    parent: Weak<RefCell<super::Controller>>,
    clear_button: Button,
    text_view: TextView,
    generation: GenerationValue,
}

impl Controller {
    pub fn new(parent: Weak<RefCell<super::Controller>>)
    -> Rc<RefCell<Controller>> {
        let window = WindowBuilder::new()
            .name("editor").type_(WindowType::Toplevel)
            .title("Tsong - Errors").build();
        let big_box = BoxBuilder::new()
            .name("errors").orientation(Orientation::Vertical)
            .build();
        window.add(&big_box);
        let text_view = TextViewBuilder::new()
            .editable(false).hexpand(true).vexpand(true).cursor_visible(false)
            .wrap_mode(WrapMode::WordChar).build();
        let scroller = ScrolledWindowBuilder::new().build();
        scroller.add(&text_view);
        big_box.add(&scroller);
        let clear_button = ButtonBuilder::new()
            .label("Clear Errors").build();
        clear_button.set_sensitive(false);
        big_box.add(&clear_button);
        let ret = Rc::new(RefCell::new(Controller {
            window, clear_button, text_view,
            parent, me: None, generation: Default::default(),
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
        this.clear_button.connect_clicked(move |_| {
            let _ = controller.try_borrow_mut()
                .map(|mut x| x.clicked_clear());
        });
        drop(this);
        ret
    }
    fn clicked_clear(&mut self) -> Option<()> {
        errors::clear_if_not_newer_than(&self.generation);
        self.populate();
        None
    }
    fn cleanup(&mut self) -> Option<()> {
        let buffer = self.text_view.get_buffer().unwrap();
        buffer.delete(&mut buffer.get_start_iter(),
                      &mut buffer.get_end_iter());
        let parent = self.parent.upgrade()?;
        parent.try_borrow_mut().ok()?.closed_errors();
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
    fn update(&mut self, generation: GenerationValue,
              errors: RwLockReadGuard<'static, BTreeMap<String,Vec<String>>>) {
        let buffer = self.text_view.get_buffer().unwrap();
        buffer.delete(&mut buffer.get_start_iter(),
                      &mut buffer.get_end_iter());
        if errors.is_empty() {
            buffer.insert(&mut buffer.get_end_iter(), "No recent errors.");
            self.clear_button.set_sensitive(false);
        }
        else {
            // For each error source...
            for (source, errors) in errors.iter() {
                if errors.len() == 1 {
                    let text = format!("<b>Error from {}:</b> {}",
                                       source, errors[0]);
                    buffer.insert_markup(&mut buffer.get_end_iter(), &text);
                }
                else {
                    let text = format!("<b>Errors from {}:<b>", source);
                    buffer.insert_markup(&mut buffer.get_end_iter(), &text);
                    // For each error from that source...
                    for error in errors.iter() {
                        buffer.insert(&mut buffer.get_end_iter(), &error);
                    }
                    buffer.insert(&mut buffer.get_end_iter(), "\n");
                }
            }
            self.clear_button.set_sensitive(true);
        }
        // before we finish, delete any extra trailing newlines
        let mut owari = buffer.get_end_iter();
        let mut koko = owari.clone();
        while koko.backward_char() && koko.ends_line() {}
        koko.forward_char();
        buffer.delete(&mut koko, &mut owari);
        self.generation = generation;
    }
    fn populate(&mut self) {
        let errors = errors::get();
        let generation = errors::generation();
        self.update(generation, errors);
    }
    pub fn update_if_visible(&mut self, generation: GenerationValue,
                             errors: RwLockReadGuard
                             <'static, BTreeMap<String,Vec<String>>>) {
        if !self.window.is_visible() { return }
        self.update(generation, errors);
    }
}
