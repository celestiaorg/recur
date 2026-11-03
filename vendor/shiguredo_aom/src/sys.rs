#![expect(non_upper_case_globals)]
#![expect(non_camel_case_types)]
// 実 bindings (bindgen 生成物) でのみ発火する lint の抑止
//
// ダミー bindings (DOCS_RS ビルド) は src/lib.rs が参照するシンボルの部分集合で、
// 命名規則違反や未使用項目を含まないため、通常ビルドでのみ抑止する。
#![cfg_attr(not(docs_rs), expect(non_snake_case))]
#![cfg_attr(not(docs_rs), expect(dead_code))]
// ダミー bindings にだけ含まれる未使用の型エイリアス用の抑止
#![cfg_attr(docs_rs, expect(unused_imports))]

include!(concat!(env!("OUT_DIR"), "/metadata.rs"));
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
