use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

fn main() {
    let allocator = Allocator::default();
    let source = "const a: number = 1; defineProps<{ msg: string }>();";
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    println!(
        "statements: {}, errors: {}",
        parsed.program.body.len(),
        parsed.diagnostics.len()
    );
}
