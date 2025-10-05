use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

extern crate proc_macro;
#[proc_macro_attribute]
pub fn metric(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(item as ItemStruct);
    let input_struct = &ast.ident;
    let expanded = quote! {
        #[derive(
            serde::Serialize,
            serde::Deserialize,
            std::cmp::PartialOrd,
            std::cmp::PartialEq,
            std::fmt::Debug,
            std::clone::Clone
        )]
        #ast

        impl Metric for #input_struct{}
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn aggregate(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(item as ItemStruct);
    let expanded = quote! {
        #[derive(
            serde::Serialize,
            serde::Deserialize,
            std::cmp::PartialOrd,
            std::cmp::PartialEq,
            std::fmt::Debug,
            std::clone::Clone
        )]
        #ast
    };

    TokenStream::from(expanded)
}
