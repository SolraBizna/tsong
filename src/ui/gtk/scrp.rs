//! Oh boy. This contains the `SensitiveCellRendererPixbuf` class.

// Well, here's my second try at subclassing a GLib object...

use gtk::{
    prelude::*,
    subclass::prelude::*,
    Allocation,
    CellRenderer,
    CellRendererMode,
    CellRendererPixbuf,
    CellRendererState
};
use glib::{
    subclass,
    translate::*,
    glib_object_impl,
    glib_object_subclass,
    glib_wrapper,
    SignalHandlerId,
    Type,
};
use gdk::{
    Event,
};

#[derive(Debug)]
pub struct SCRPPrivate {
}

impl ObjectSubclass for SCRPPrivate {
    const NAME: &'static str = "SensitiveCellRendererPixbuf";
    type ParentType = CellRendererPixbuf;
    type Instance = subclass::simple::InstanceStruct<Self>;
    type Class = subclass::simple::ClassStruct<Self>;
    glib_object_subclass!();
    fn class_init(klass: &mut Self::Class) {
        klass.add_signal(
            "clicked",
            glib::SignalFlags::RUN_LAST,
            &[Type::String],
            Type::Bool,
        );
    }
    fn new() -> Self {
        Self {}
    }
}

impl ObjectImpl for SCRPPrivate {
    glib_object_impl!();
}

impl CellRendererPixbufImpl for SCRPPrivate {}
impl CellRendererImpl for SCRPPrivate {
    fn activate<P: IsA<gtk::Widget>>(&self, me: &CellRenderer,
                                     _: Option<&Event>, _: &P, path: &str,
                                     _: &Allocation, _: &Allocation,
                                     _: CellRendererState) -> bool {
        match me.emit("clicked", &[&path]) {
            Err(_) => false,
            Ok(None) => false,
            Ok(Some(x)) => x.get().unwrap_or(None).unwrap_or(false),
        }
    }
}

glib_wrapper! {
    pub struct SensitiveCellRendererPixbuf(
        Object<subclass::simple::InstanceStruct<SCRPPrivate>,
               subclass::simple::ClassStruct<SCRPPrivate>,
               SensitiveCellRendererPixbufClass>)
        @extends CellRendererPixbuf, CellRenderer;
    match fn {
        get_type => || SCRPPrivate::get_type().to_glib(),
    }
}

impl SensitiveCellRendererPixbuf {
    pub fn new() -> SensitiveCellRendererPixbuf {
        glib::Object::new(Self::static_type(),
                          &[("mode", &CellRendererMode::Activatable)])
            .unwrap().downcast().unwrap()
    }
    pub fn connect_clicked<F: Fn(&str) -> bool + 'static>(&self, f: F)
    -> SignalHandlerId {
        // that bold red text makes me unhappy, but the alternative is an
        // explicit freaking trampoline, itself marked unsafe, and a slightly
        // larger unsafe block...
        unsafe {
            self.connect_unsafe("clicked", true, move |values| {
                // that's a lot of wrapping paper
                let path = values.get(1).unwrap().get().unwrap().unwrap();
                Some(f(path).to_value())
            }).unwrap()
        }
    }
}
