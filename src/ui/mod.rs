mod gtk;

pub fn go() -> ! {
    gtk::go();
    std::process::exit(0)
}
