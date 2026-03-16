use docgen::{construct_anchor_cli_tree, generate_markdown};

fn main() {
    let cmd = construct_anchor_cli_tree();
    let docs = generate_markdown(&cmd);
    print!("{docs}");
}
