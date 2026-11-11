//! Dev utility: print the assembled `:help` manual (contents + every chapter,
//! including the generated COMMANDS chapter) for width-eyeballing.
fn main() {
    let commands = shannon_tui::command::commands_chapter();
    print!("{}", shannon_tui::help::compose(&commands));
}
