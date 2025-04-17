use crate::database::DatabaseExt;
use crate::query::QueryMacroInput;
use either::Either;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use sqlx_core::{describe::Describe, type_info::TypeInfo};
use syn::spanned::Spanned;

/// Returns a tokenstream which typechecks the arguments passed to the macro
/// and binds them to `DB::Arguments` with the ident `query_args`.
pub fn quote_args<DB: DatabaseExt>(
    input: &QueryMacroInput,
    info: &Describe<DB>,
) -> crate::Result<TokenStream> {
    let db_path = DB::db_path();

    if input.arg_exprs.is_empty() {
        return Ok(quote! {
            let query_args = ::core::result::Result::<_, ::sqlx::error::BoxDynError>::Ok(<#db_path as ::sqlx::database::Database>::Arguments::<'_>::default());
        });
    }

    let arg_names = (0..input.arg_exprs.len())
        .map(|i| format_ident!("arg{}", i))
        .collect::<Vec<_>>();

    let Some(Either::Left(params)) = info.parameters() else {
        unimplemented!("only normal parameter inputs are supported safely");
    };

    let params = params
        .iter()
        .map(|param| {
            let maybe_real_type = DB::param_type_for_id(param);
            let known_enum_type = info.known_enum_tys.get(param.name());

            match (maybe_real_type, known_enum_type) {
                (Some(rt), _) => rt.parse::<TokenStream>().map_err(|err| {
                    format!("failed to parse parameter type `{param}`: {err}").into()
                }),
                (None, Some(et)) => {
                    // if we have an enum, we can coerce it into a string.
                    // TODO: add a trait that we actually require here
                    ephemeral_enum_ty(param.name(), et)
                }
                _ => Err(format!("parameter type `{param}` is not supported").into()),
            }
        })
        .collect::<crate::Result<Vec<_>>>()?;

    let arg_bindings = input
        .arg_exprs
        .iter()
        .cloned()
        .zip(params.iter())
        .zip(&arg_names)
        .map(|((expr, param), arg_name)| -> TokenStream {
            quote_spanned!(expr.span() =>
                // TODO: make something like `sqlx::DbInto` so that these from impls can be
                // disambiguated from any other from impl
                let #arg_name = &(<_ as ::core::convert::Into<#param>>::into(#expr));
            )
        })
        .collect::<TokenStream>();

    let args_check = params
        .iter()
        .zip(arg_names.iter().zip(&input.arg_exprs))
        .map(|(param_ty, (name, expr))| -> crate::Result<_> {
            Ok(quote_spanned!(expr.span() =>
                // this shouldn't actually run
                #[allow(clippy::missing_panics_doc, clippy::unreachable)]
                if false {
                    use ::sqlx::ty_match::{WrapSameExt as _, MatchBorrowExt as _};

                    // evaluate the expression only once in case it contains moves
                    let expr = ::sqlx::ty_match::dupe_value(#name);

                    // if `expr` is `Option<T>`, get `Option<$ty>`, otherwise `$ty`
                    let ty_check = ::sqlx::ty_match::WrapSame::<#param_ty, _>::new(&expr).wrap_same();

                    // if `expr` is `&str`, convert `String` to `&str`
                    let (mut _ty_check, match_borrow) = ::sqlx::ty_match::MatchBorrow::new(ty_check, &expr);

                    _ty_check = match_borrow.match_borrow();

                    // this causes move-analysis to effectively ignore this block
                    ::std::unreachable!();
                }
        ))})
        .collect::<crate::Result<TokenStream>>()?;

    let args_count = input.arg_exprs.len();

    Ok(quote! {
        #arg_bindings

        #args_check

        let mut query_args = <#db_path as ::sqlx::database::Database>::Arguments::<'_>::default();
        query_args.reserve(
            #args_count,
            0 #(+ ::sqlx::encode::Encode::<#db_path>::size_hint(#arg_names))*
        );
        let query_args = ::core::result::Result::<_, ::sqlx::error::BoxDynError>::Ok(query_args)
        #(.and_then(move |mut query_args| query_args.add(#arg_names).map(move |()| query_args) ))*;
    })
}

fn ephemeral_enum_ty(name: &str, args: &[String]) -> crate::Result<TokenStream> {
    let enum_name = format_ident!("{name}");

    //     Ok(quote! {
    //         pub enum #enum_name {
    //             #(
    //                 #[sqlx(rename = #args)]
    //                 #args,
    //             )*
    //         }
    //
    //         impl ::core::convert::From<#enum_name> for ::std::string::String {
    //             fn from(value: #enum_name) -> Self {
    //                 match value {
    //                     #(
    //                         #enum_name::#args => #args.to_string(),
    //                     )*
    //                 }
    //             }
    //         }
    //     })

    Ok(quote! {
        ::std::string::String
    })
}
