//! Compile-time classification of diagnostic log values.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use std::collections::BTreeMap;
use syn::{parse::Parse, parse::ParseStream, Expr, Ident, LitStr, Token};

struct Input {
    template: LitStr,
    args: Vec<Expr>,
}

impl Parse for Input {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let template = input.parse()?;
        let mut args = Vec::new();
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if !input.is_empty() {
                args.push(input.parse()?);
            }
        }
        Ok(Self { template, args })
    }
}

#[proc_macro]
pub fn line(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as Input);
    expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro]
pub fn log(input: TokenStream) -> TokenStream {
    let line = proc_macro2::TokenStream::from(line(input));
    quote!({
        let line = #line;
        ::lucidos_diagnostic_log::record(module_path!(), line);
    })
    .into()
}

fn expand(input: Input) -> syn::Result<proc_macro2::TokenStream> {
    let template = input.template;
    let mut values = Vec::new();
    let mut slots = BTreeMap::new();
    let mut positional = 0;
    for arg in &input.args {
        let (key, expr) = if let Expr::Assign(assign) = arg {
            let Expr::Path(path) = &*assign.left else {
                return Err(syn::Error::new_spanned(
                    arg,
                    "expected a named log argument",
                ));
            };
            (quote!(#path).to_string(), &*assign.right)
        } else {
            let key = positional.to_string();
            positional += 1;
            (key, arg)
        };
        if slots.contains_key(&key) {
            return Err(syn::Error::new_spanned(arg, "duplicate log argument"));
        }
        let local = format_ident!("__diagnostic_arg_{}", values.len());
        slots.insert(key, local.clone());
        values.push(quote!(let #local = &(#expr);));
    }
    let mut fragments = Vec::new();
    let text = template.value();
    let mut chars = text.chars().peekable();
    let mut literal = String::new();
    let mut next = 0;
    while let Some(c) = chars.next() {
        if (c == '{' || c == '}') && chars.peek() == Some(&c) {
            chars.next();
            literal.push(c);
        } else if c == '{' {
            fragments.push(quote!(::lucidos_diagnostic_log::Fragment::literal(#literal)));
            literal.clear();
            let mut field = String::new();
            loop {
                match chars.next() {
                    Some('}') => break,
                    Some(c) => field.push(c),
                    None => {
                        return Err(syn::Error::new_spanned(
                            &template,
                            "unclosed log placeholder",
                        ))
                    }
                }
            }
            let (selector, spec) = field.split_once(':').unwrap_or((&field, ""));
            let mut spec = spec.to_string();
            let mut dynamic = Vec::new();
            if spec.contains(".*") {
                let key = next.to_string();
                next += 1;
                let value = resolve(&key, &slots)?;
                dynamic.push(quote!(__diagnostic_precision = #value));
                spec = spec.replace(".*", ".__diagnostic_precision$");
            }
            let selector = if selector.is_empty() {
                let key = next.to_string();
                next += 1;
                key
            } else {
                selector.to_string()
            };
            let value = resolve(&selector, &slots)?;
            // Width and precision arguments carry values too, even if no text
            // placeholder names them directly.
            let mut rewritten = String::new();
            let mut word = String::new();
            for c in spec.chars() {
                if c.is_alphanumeric() || c == '_' {
                    word.push(c);
                } else {
                    if c == '$' && word != "__diagnostic_precision" {
                        let selector = match word.strip_prefix('0').filter(|rest| !rest.is_empty())
                        {
                            Some(rest) => {
                                rewritten.push('0');
                                rest
                            }
                            None => &word,
                        };
                        let width = resolve(selector, &slots)?;
                        let name = format_ident!("__diagnostic_width_{}", dynamic.len());
                        values.push(quote!(::lucidos_diagnostic_log::check_value(#width);));
                        dynamic.push(quote!(#name = #width));
                        word = name.to_string();
                    }
                    rewritten.push_str(&word);
                    word.clear();
                    rewritten.push(c);
                }
            }
            rewritten.push_str(&word);
            let fmt = LitStr::new(&format!("{{0:{rewritten}}}"), Span::call_site());
            fragments.push(quote!(::lucidos_diagnostic_log::Fragment::value(
                #value, format!(#fmt, #value, #(#dynamic),*)
            )));
        } else if c == '}' {
            return Err(syn::Error::new_spanned(
                &template,
                "unmatched closing brace",
            ));
        } else {
            literal.push(c);
        }
    }
    fragments.push(quote!(::lucidos_diagnostic_log::Fragment::literal(#literal)));
    let checks = slots
        .values()
        .map(|value| quote!(::lucidos_diagnostic_log::check_value(#value);));
    Ok(quote!({
        #(#values)*
        #(#checks)*
        ::lucidos_diagnostic_log::Line::typed(#template, vec![#(#fragments),*])
    }))
}

fn resolve(
    selector: &str,
    slots: &BTreeMap<String, Ident>,
) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(local) = slots.get(selector) {
        Ok(quote!(#local))
    } else {
        let capture = syn::parse_str::<Ident>(selector)
            .map_err(|_| syn::Error::new(Span::call_site(), "missing log argument"))?;
        Ok(quote!(&#capture))
    }
}
