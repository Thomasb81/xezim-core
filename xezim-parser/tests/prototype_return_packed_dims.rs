//! #284: IEEE 1800-2023 A.2.2.1 `data_type ::= ... | [ class_scope |
//! package_scope ] type_identifier { packed_dimension }`. A function
//! PROTOTYPE (`extern`, `pure virtual`, interface-class and DPI forms) takes
//! the same return type as a function declaration, so `M [1:0]` is accepted
//! there too; it used to stop at the `[`. An implicit return type with
//! dimensions only (`[7:0]`) is accepted the same way.

use sv_parser::ast::Description;
use sv_parser::ast::decl::{ClassItem, ClassMethodKind};
use sv_parser::ast::types::DataType;
use sv_parser::parse;

#[test]
fn prototypes_take_typedef_return_with_packed_dims() {
    let r = parse(
        r#"
typedef bit [31:0] M;
interface class IC;
  pure virtual function M [1:0] icf(int k);
endclass
virtual class VB;
  pure virtual function M [3:0] pv(input int k);
endclass
class c;
  extern virtual function M [1:0] low_two(bit [511:0] w);
  extern local function M [1:0] loc_f();
  extern static function M [0:0] st_f();
  extern function [7:0] imp_f();
  extern task tk(output M [1:0] o);
endclass
function M [1:0] c::low_two(bit [511:0] w);
  return 64'hdead_beef_cafe_f00d;
endfunction
module top;
  import "DPI-C" function M [1:0] dpi_two(int k);
  export "DPI-C" function sv_two;
  function M [1:0] sv_two(int k);
    return 0;
  endfunction
endmodule
"#,
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let class_c = r
        .source
        .descriptions
        .iter()
        .find_map(|d| match d {
            Description::Class(c) if c.name.name == "c" => Some(c),
            _ => None,
        })
        .expect("class c");
    let dims: Vec<(String, usize)> = class_c
        .items
        .iter()
        .filter_map(|it| match it {
            ClassItem::Method(m) => match &m.kind {
                ClassMethodKind::Extern(f) => Some(f),
                _ => None,
            },
            _ => None,
        })
        .map(|f| {
            let n = match &f.return_type {
                DataType::TypeReference { dimensions, .. } => dimensions.len(),
                DataType::Implicit { dimensions, .. } => dimensions.len(),
                _ => usize::MAX,
            };
            (f.name.name.name.clone(), n)
        })
        .collect();
    assert_eq!(
        dims,
        [
            ("low_two".to_string(), 1),
            ("loc_f".to_string(), 1),
            ("st_f".to_string(), 1),
            ("imp_f".to_string(), 1),
        ]
    );
}
