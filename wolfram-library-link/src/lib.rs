//! A safe and convenient wrapper around Wolfram [LibraryLink][library-link-guide].
//!
//! Wolfram LibraryLink is framework for authoring dynamic libraries that can be
//! [dynamically loaded][library-function-load] by the [Wolfram Language][WL]. This crate
//! provides idiomatic Rust bindings around the lower-level LibraryLink C interface.
//!
//! This library provides functionality for:
//!
//! * Calling Rust functions from the Wolfram Language.
//! * Passing data efficiently to and from the Wolfram Language using native data types
//!   like [`NumericArray`] and [`Image`].
//! * Passing arbitrary expressions to and from the Wolfram Language using
//!   [`Expr`][struct@crate::Expr] and the [`export_wstp!`][crate::export_wstp]
//!   macro.
//! * Asynchronous events handled by the Wolfram Language, generated using a background
//!   thread spawned via [`AsyncTaskObject`].
//!
//! #### Related Links
//!
//! * [*LibraryLink for Rust Quick Start*][QuickStart] &nbsp;&nbsp;<small>[[TODO: Update to public link]]</small>
//! * [*Wolfram LibraryLink User Guide*](https://reference.wolfram.com/language/LibraryLink/tutorial/Overview.html)
//!
//! # Examples
//!
//! Writing a Rust function that can be called from the Wolfram Language is as easy as
//! writing:
//!
//! ```
//! # mod scope {
//! use wolfram_library_link::export;
//!
//! export![square(_)];
//!
//! fn square(x: i64) -> i64 {
//!     x * x
//! }
//! # }
//! ```
//!
//! [building your dynamic library][QuickStart], and loading the function into the Wolfram Language
//! using [`LibraryFunctionLoad`][library-function-load]:
//!
//! ```wolfram
//! func = LibraryFunctionLoad["library_name", "square", {Integer}, Integer];
//!
//! func[5]   (* Returns 25 *)
//! ```
//!
//! [QuickStart]: https://stash.wolfram.com/users/connorg/repos/rustlink/browse/docs/QuickStart.md
//!
//! ## Show backtrace when a panic occurs
//!
//! Functions backed by a WSTP [`Link`] (using [`export_wstp![]`][crate::export_wstp]) will
//! automatically catch any
//! Rust panics that occur in the wrapped code, and return a [`Failure`][failure] object
//! with the panic message and source file/line number. It also can optionally show the
//! backtrace. This is configured by the `"LIBRARY_LINK_RUST_BACKTRACE"` environment
//! variable. Enable it by evaluating:
//!
//! ```wolfram
//! SetEnvironment["LIBRARY_LINK_RUST_BACKTRACE" -> "True"]
//! ```
//!
//! Now the error shown when a panic occurs will include a backtrace.
//!
//! Note that the error message may include more information if the `"nightly"`
//! [feature][cargo-features] of `wolfram-library-link` is enabled.
//!
//! [WL]: https://wolfram.com/language
//! [library-link-guide]: https://reference.wolfram.com/language/guide/LibraryLink.html
//! [library-function-load]: https://reference.wolfram.com/language/ref/LibraryFunctionLoad.html
//! [failure]: https://reference.wolfram.com/language/ref/Failure.html
//! [cargo-features]: https://doc.rust-lang.org/cargo/reference/features.html
// #![doc = include_str!("../docs/included/Overview.md")]
#![cfg_attr(feature = "nightly", feature(panic_info_message))]
#![warn(missing_docs)]

mod args;
mod async_tasks;
mod catch_panic;
mod data_store;
mod image;
mod library_data;
/// This module is *semver exempt*. This is not intended to be part of the public API of
/// wolfram-library-link.
///
/// Utilities used by code generated by the public macros.
#[doc(hidden)]
pub mod macro_utils;
mod numeric_array;
pub mod rtl;


/// Wolfram Language expressions.
//
// Note: This is exported as doc(inline) so that it shows up in the 'Modules' section of
//       the crate docs instead of in the 'Re-exports' section. This is to make way for
//       the chance that in the future, wolfram-library-link will have it's own expression
//       type that uses types like NumericArray and Image as variants, which can't be
//       used in the more general wolfram-expr crate (since NumericArray and Image depend
//       on the Wolfram RTL, which isn't available in arbitrary Rust code).
#[doc(inline)]
pub use wl_expr_core as expr;
pub use wolfram_library_link_sys as sys;
pub use wstp;

// Used by the export!/export_wstp! macro implementations.
#[doc(hidden)]
pub use inventory;

pub use self::{
    args::{FromArg, IntoArg, NativeFunction, WstpFunction},
    async_tasks::AsyncTaskObject,
    data_store::{DataStore, DataStoreNode, DataStoreNodeValue, Nodes},
    image::{ColorSpace, Image, ImageData, ImageType, Pixel, UninitImage},
    library_data::{get_library_data, initialize, WolframLibraryData},
    numeric_array::{
        NumericArray, NumericArrayConvertMethod, NumericArrayDataType, NumericArrayKind,
        NumericArrayType, UninitNumericArray,
    },
};


use std::sync::Mutex;

use once_cell::sync::Lazy;

use wolfram_library_link_sys::mint;
use wstp::Link;

pub(crate) use self::library_data::assert_main_thread;
use crate::expr::{Expr, ExprKind, Symbol};


const BACKTRACE_ENV_VAR: &str = "LIBRARY_LINK_RUST_BACKTRACE";

//======================================
// Callbacks to the Wolfram Kernel
//======================================

/// Evaluate `expr` by calling back into the Wolfram Kernel.
///
/// TODO: Specify and document what happens if the evaluation of `expr` triggers a
///       kernel abort (such as a `Throw[]` in the code).
pub fn evaluate(expr: &Expr) -> Expr {
    match try_evaluate(expr) {
        Ok(returned) => returned,
        Err(msg) => panic!(
            "evaluate(): evaluation of expression failed: {}: \n\texpression: {}",
            msg, expr
        ),
    }
}

/// Attempt to evaluate `expr`, returning an error if a WSTP transport error occurred
/// or evaluation failed.
pub fn try_evaluate(expr: &Expr) -> Result<Expr, String> {
    with_link(|link: &mut Link| {
        // Send an EvaluatePacket['expr].
        let _: () = link
            // .put_expr(&Expr! { EvaluatePacket['expr] })
            .put_expr(&Expr::normal(
                Symbol::new("System`EvaluatePacket").unwrap(),
                vec![expr.clone()],
            ))
            .map_err(|e| e.to_string())?;

        let _: () = process_wstp_link(link)?;

        let return_packet: Expr = link.get_expr().map_err(|e| e.to_string())?;

        let returned_expr = match return_packet.kind() {
            ExprKind::Normal(normal) => {
                debug_assert!(
                    normal.has_head(&Symbol::new("System`ReturnPacket").unwrap())
                );
                debug_assert!(normal.contents.len() == 1);
                normal.contents[0].clone()
            },
            _ => {
                return Err(format!(
                    "try_evaluate(): returned expression was not ReturnPacket: {}",
                    return_packet
                ))
            },
        };

        Ok(returned_expr)
    })
}

/// Returns `true` if the user has requested that the current evaluation be aborted.
///
/// Programs should finish what they are doing and return control of this thread to
/// to the kernel as quickly as possible. They should not exit the process or
/// otherwise terminate execution, simply return up the call stack.
///
/// Within Rust functions exported using [`export!`][crate::export] or
/// [`export_wstp!`][export_wstp!] (which generate a wrapper function that catches panics),
/// `panic!()` can be used to quickly unwind the call stack to the appropriate place.
/// Note that this will not work if the current library is built with
/// `panic = "abort"`. See the [`panic`][panic-option] profile configuration option
/// for more information.
///
/// [panic-option]: https://doc.rust-lang.org/cargo/reference/profiles.html#panic
pub fn aborted() -> bool {
    // TODO: Is this function thread safe? Can it be called from a thread other than the
    //       one the LibraryLink wrapper was originally invoked from?
    let val: mint = unsafe { rtl::AbortQ() };
    // TODO: What values can `val` be?
    val == 1
}

fn process_wstp_link(link: &mut Link) -> Result<(), String> {
    assert_main_thread();

    let raw_link = unsafe { link.raw_link() };

    // Process the packet on the link.
    let code: i32 = unsafe { rtl::processWSLINK(raw_link as *mut _) };

    if code == 0 {
        let error_message = link
            .error_message()
            .unwrap_or_else(|| "unknown error occurred on WSTP Link".into());

        return Err(error_message);
    }

    Ok(())
}

/// Enforce exclusive access to the link returned by `getWSLINK()`.
fn with_link<F: FnOnce(&mut Link) -> R, R>(f: F) -> R {
    assert_main_thread();

    static LOCK: Lazy<Mutex<()>> = Lazy::new(|| Default::default());

    let _guard = LOCK.lock().expect("failed to acquire LINK lock");

    let lib = get_library_data().raw_library_data;

    let unsafe_link: sys::WSLINK = unsafe { rtl::getWSLINK(lib) };
    let mut unsafe_link: wstp::sys::WSLINK = unsafe_link as wstp::sys::WSLINK;

    // Safety:
    //      By using LOCK to ensure exclusive access to the `getWSLINK()` value within
    //      safe code, we can be confident that this `&mut Link` will not alias with
    //      other references to the underling link object.
    let link = unsafe { Link::unchecked_ref_cast_mut(&mut unsafe_link) };

    f(link)
}

#[inline]
fn bool_from_mbool(boole: sys::mbool) -> bool {
    boole != 0
}

/// Export the specified functions as native *LibraryLink* functions.
///
/// To be exported by this macro, the specified function(s) must implement
/// [`NativeFunction`].
///
/// Functions exported using this macro will automatically:
///
/// * Call [`initialize()`] to initialize this library.
/// * Catch any panics that occur.
///   - If a panic does occur, the function will return
///     [`LIBRARY_FUNCTION_ERROR`][crate::sys::LIBRARY_FUNCTION_ERROR].
///
// * Extract the function arguments from the raw [`MArgument`] array.
// * Store the function return value in the raw [`MArgument`] return value field.
///
/// # Syntax
///
/// Export a function with a single argument.
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::export;
/// # fn square(x: i64) -> i64 { x }
/// export![square(_)];
/// # }
/// ```
///
/// Export a function using the specified low-level shared library symbol name.
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::export;
/// # fn square(x: i64) -> i64 { x }
/// export![square(_) as WL_square];
/// # }
/// ```
///
/// Export multiple functions with one `export!` invocation. This is purely for convenience.
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::export;
/// # fn square(x: i64) -> i64 { x }
/// # fn add_two(a: i64, b: i64) -> i64 { a + b }
/// export![
///     square(_);
///     add_two(_, _) as AddTwo;
/// ];
/// # }
/// ```
///
// TODO: Remove this feature? If someone wants to export the low-level function, they
//       should do `pub use square::square as ...` instead of exposing the hidden module
//       (which is just an implementation detail of `export![]` anyway).
// Make public the `mod` module that contains the low-level wrapper function.
//
// ```
// export![pub square(_)];
// ```
///
/// # Examples
///
/// ### Primitive data types
///
/// Export a native function with a single integer argument:
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::export;
/// fn square(x: i64) -> i64 {
///     x * x
/// }
///
/// export![square(_)];
/// # }
/// ```
///
/// ```wolfram
/// LibraryFunctionLoad["...", "square", {Integer}, Integer]
/// ```
///
/// Export a native function with a single string argument:
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::export;
/// fn reverse_string(string: String) -> String {
///     string.chars().rev().collect()
/// }
///
/// export![reverse_string(_)];
/// # }
/// ```
///
/// ```wolfram
/// LibraryFunctionLoad["...", "reverse_string", {String}, String]
/// ```
///
/// Export a native function with multiple arguments:
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::export;
/// fn times(a: f64, b: f64) -> f64 {
///     a * b
/// }
///
/// export![times(_, _)];
/// # }
/// ```
///
/// ```wolfram
/// LibraryFunctionLoad["...", "times", {Real, Real}, Real]
/// ```
///
/// ### Numeric arrays
///
/// Export a native function with a [`NumericArray`] argument:
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::{export, NumericArray};
/// fn total_i64(list: &NumericArray<i64>) -> i64 {
///     list.as_slice().into_iter().sum()
/// }
///
/// export![total_i64(_)];
/// # }
/// ```
///
/// ```wolfram
/// LibraryFunctionLoad[
///     "...", "total_i64",
///     {LibraryDataType[NumericArray, "Integer64"]}
///     Integer
/// ]
/// ```
///
///
// TODO: Add a "Memory Management" section to this comment and discuss "Constant".
//
// ```wolfram
// LibraryFunctionLoad[
//     "...", "total_i64",
//     {
//         {LibraryDataType[NumericArray, "Integer64"], "Constant"}
//     },
//     Integer
// ]
// ```
///
/// # Parameter types
///
/// When manually writing the Wolfram
/// [`LibraryFunctionLoad`][ref/LibraryFunctionLoad]<sub>WL</sub> call necessary to load
/// a Rust *LibraryLink* function, you must declare the type signature of the function
/// using the appropriate types.
///
/// The following table describes the relationship between Rust types that can be used as
/// parameter types in a native LibraryLink function (namely: those that implement
/// [`FromArg`]) and the compatible Wolfram *LibraryLink* function parameter type(s).
///
/// [`FromArg::parameter_type()`] can be used to determine the Wolfram library function
/// parameter type programatically.
///
/// If you would prefer to have the Wolfram Language code for loading your library be
/// generated automatically, use the [`generate_loader!`] macro.
///
/// <h4 style="border-bottom: none; margin-bottom: 4px"> ⚠️ Warning! ⚠️ </h4>
///
/// Calling a *LibraryLink* function from the Wolfram Language that was loaded using the
/// wrong parameter type may lead to undefined behavior! Ensure that the function
/// parameter type declared in your Wolfram Language code matches the Rust function
/// parameter type.
///
/// Rust parameter type                | Wolfram library function parameter type
/// -----------------------------------|---------------------------------------
/// [`bool`]                           | `"Boolean"`
/// [`mint`]                           | `Integer`
/// [`mreal`][crate::sys::mreal]       | `Real`
/// [`mcomplex`][crate::sys::mcomplex] | `Complex`
/// [`String`]                         | `String`
/// [`CString`][std::ffi::CString]     | `String`
/// [`&NumericArray`][NumericArray]    | a. `LibraryDataType[NumericArray]` <br/> b. `{LibraryDataType[NumericArray], "Constant"}`[^1]
/// [`NumericArray`]                   | a. `{LibraryDataType[NumericArray], "Manual"}`[^1] <br/> b. `{LibraryDataType[NumericArray], "Shared"}`[^1]
/// [`&NumericArray<T>`][NumericArray] | a. `LibraryDataType[NumericArray, `[`"..."`][ref/NumericArray]`]`[^1] <br/> b. `{LibraryDataType[NumericArray, "..."], "Constant"}`[^1]
/// [`NumericArray<T>`]                | a. `{LibraryDataType[NumericArray, "..."], "Manual"}`[^1] <br/> b. `{LibraryDataType[NumericArray, "..."], "Shared"}`[^1]
/// [`DataStore`]                      | `"DataStore"`
///
/// # Return types
///
/// The following table describes the relationship between Rust types that implement
/// [`IntoArg`] and the compatible Wolfram *LibraryLink* function return type.
///
/// [`IntoArg::return_type()`] can be used to determine the Wolfram library function
/// parameter type programatically.
///
/// Rust return type                   | Wolfram library function return type
/// -----------------------------------|---------------------------------------
/// [`()`][`unit`]                     | `"Void"`
/// [`bool`]                           | `"Boolean"`
/// [`mint`]                           | `Integer`
/// [`mreal`][crate::sys::mreal]       | `Real`
/// [`i8`], [`i16`], [`i32`]           | `Integer`
/// [`u8`], [`u16`], [`u32`]           | `Integer`
/// [`f32`]                            | `Real`
/// [`mcomplex`][crate::sys::mcomplex] | `Complex`
/// [`String`]                         | `String`
/// [`NumericArray`]                   | `LibraryDataType[NumericArray]`
/// [`NumericArray<T>`]                | `LibraryDataType[NumericArray, `[`"..."`][ref/NumericArray][^1]`]`
/// [`DataStore`]                      | `"DataStore"`
///
/// [^1]: The Details and Options section of the Wolfram Language
///       [`NumericArray` reference page][ref/NumericArray] lists the available element
///       types.
///
/// [ref/NumericArray]: https://reference.wolfram.com/language/ref/NumericArray.html
/// [ref/LibraryFunctionLoad]: https://reference.wolfram.com/language/ref/LibraryFunctionLoad.html

// # Design constraints
//
// The current design of this macro is intended to accommodate the following constraints:
//
// 1. Support automatic generation of wrapper functions without using procedural macros,
//    and with minimal code duplication. Procedural macros require external dependencies,
//    and can significantly increase compile times.
//
//      1a. Don't depend on the entire function definition to be contained within the
//          macro invocation, which leads to unergonomic rightward drift. E.g. don't
//          require something like:
//
//          export![
//              fn foo(x: i64) { ... }
//          ]
//
//      1b. Don't depend on the entire function declaration to be repeated in the
//          macro invocation. E.g. don't require:
//
//              fn foo(x: i64) -> i64 {...}
//
//              export![
//                  fn foo(x: i64) -> i64;
//              ]
//
// 2. The name of the function in Rust should match the name of the function that appears
//    in the WL LibraryFunctionLoad call. E.g. needing different `foo` and `foo__wrapper`
//    named must be avoided.
//
// To satisfy constraint 1, it's necessary to depend on the type system rather than
// clever macro operations. This leads naturally to the creation of the `NativeFunction`
// trait, which is implemented for all suitable `fn(..) -> _` types.
//
// Constraint 1b is unable to be met completely by the current implementation due to
// limitations with Rust's coercion from `fn(A, B, ..) -> C {some_name}` to
// `fn(A, B, ..) -> C`. The coercion requires that the number of parameters (`foo(_, _)`)
// be made explicit, even if their types can be elided. If eliding the number of fn(..)
// arguments were permitted, `export![foo]` could work.
//
// To satisfy constraint 2, this implementation creates a private module with the same
// name as the function that is being wrapped. This is required because in Rust (as in
// many languages), it's illegal for two different functions with the same name to exist
// within the same module:
//
// ```
// fn foo { ... }
//
// #[no_mangle]
// pub extern "C" fn foo { ... } // Error: conflicts with the other foo()
// ```
//
// This means that the export![] macro cannot simply generate a wrapper function
// with the same name as the wrapped function, because they would conflict.
//
// However, it *is* legal for a module to contain a function and a child module that
// have the same name. Because `#[no_mangle]` functions are exported from the crate no
// matter where they appear in the module heirarchy, this offers an effective workaround
// for the name clash issue, while satisfy constraint 2's requirement that the original
// function and the wrapper function have the same name:
//
// ```
// fn foo() { ... } // This does not conflict with the `foo` module.
//
// mod foo {
//     #[no_mangle]
//     pub extern "C" fn foo(..) { ... } // This does not conflict with super::foo().
// }
// ```
#[macro_export]
macro_rules! export {
    ($vis:vis $name:ident($($argc:ty),*) as $exported:ident) => {
        $vis mod $name {
            #[no_mangle]
            pub unsafe extern "C" fn $exported(
                lib: $crate::sys::WolframLibraryData,
                argc: $crate::sys::mint,
                args: *mut $crate::sys::MArgument,
                res: $crate::sys::MArgument,
            ) -> std::os::raw::c_uint {
                // Cast away the unique `fn(...) {some_name}` function type to get the
                // generic `fn(...)` type.
                // The number of `$argc` is required for type inference of the variadic
                // `fn(..) -> _` type to work. See constraint 2a.
                let func: fn($($argc),*) -> _ = super::$name;

                $crate::macro_utils::call_native_wolfram_library_function(
                    lib,
                    args,
                    argc,
                    res,
                    func
                )
            }
        }

        // Register this exported function.
        $crate::inventory::submit! {
            $crate::macro_utils::LibraryLinkFunction::Native {
                name: stringify!($exported),
                signature: || {
                    let func: fn($($argc),*) -> _ = $name;
                    let func: &dyn $crate::NativeFunction<'_> = &func;

                    func.signature()
                }
            }
        }
    };

    // Convert export![name(..)] to export![name(..) as name].
    ($vis:vis $name:ident($($argc:ty),*)) => {
        $crate::export![$vis $name($($argc),*) as $name];
    };

    ($($vis:vis $name:ident($($argc:ty),*) $(as $exported:ident)?);* $(;)?) => {
        $(
            $crate::export![$vis $name($($argc),*) $(as $exported)?];
        )*
    };
}

/// Export the specified functions as native *LibraryLink* WSTP functions.
///
/// To be exported by this macro, the specified function(s) must implement
/// [`WstpFunction`].
///
/// Functions exported using this macro will automatically:
///
/// * Call [`initialize()`][crate::initialize] to initialize this library.
/// * Catch any panics that occur.
///   - If a panic does occur, it will be returned as a [`Failure[...]`][ref/Failure]
///     expression.
///
/// [ref/Failure]: https://reference.wolfram.com/language/ref/Failure.html
///
/// # Syntax
///
/// Export a LibraryLink WSTP function.
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::{export_wstp, wstp::Link};
/// # use wl_expr_core::Expr;
/// # fn square(args: Vec<Expr>) -> Expr { todo!() }
/// export_wstp![square(_)];
/// # }
/// ```
///
/// Export a LibraryLink WSTP function using the specified low-level shared library symbol
/// name.
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::{export_wstp, wstp::Link};
/// # use wl_expr_core::Expr;
/// # fn square(args: Vec<Expr>) -> Expr { todo!() }
/// export_wstp![square(_) as WL_square];
/// # }
/// ```
///
/// Export multiple functions with one `export_wstp!` invocation. This is purely for
/// convenience.
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::{export_wstp, wstp::Link};
/// # use wl_expr_core::Expr;
/// # fn square(args: Vec<Expr>) { }
/// # fn add_two(args: Vec<Expr>) { }
/// export_wstp![
///     square(_);
///     add_two(_) as AddTwo;
/// ];
/// # }
/// ```
///
// TODO: Remove this feature? If someone wants to export the low-level function, they
//       should do `pub use square::square as ...` instead of exposing the hidden module
//       (which is just an implementation detail of `export![]` anyway).
// Make public the `mod` module that contains the low-level wrapper function.
//
// ```
// export![pub square(_)];
// ```
///
/// # Examples
///
/// ##### WSTP function that squares a single integer argument:
///
/// ```
/// # mod scope {
/// use wolfram_library_link::{export_wstp, wstp::Link};
///
/// fn square_wstp(link: &mut Link) {
///     // Get the number of elements in the arguments list.
///     let arg_count = link.test_head("List").unwrap();
///
///     if arg_count != 1 {
///         panic!("square_wstp: expected to get a single argument");
///     }
///
///     // Get the argument value.
///     let x = link.get_i64().expect("expected Integer argument");
///
///     // Write the return value.
///     link.put_i64(x * x).unwrap();
/// }
///
/// export_wstp![square_wstp(&mut Link)];
/// # }
/// ```
///
/// ```wolfram
/// LibraryFunctionLoad["...", "square_wstp", LinkObject, LinkObject]
/// ```
///
/// ##### WSTP function that computes the sum of a variable number of arguments:
///
/// ```
/// # mod scope {
/// use wolfram_library_link::{export_wstp, wstp::Link};
///
/// fn total_args_i64(link: &mut Link) {
///     // Check that we recieved a functions arguments list, and get the number of arguments.
///     let arg_count: usize = link.test_head("List").unwrap();
///
///     let mut total: i64 = 0;
///
///     // Get each argument, assuming that they are all integers, and add it to the total.
///     for _ in 0..arg_count {
///         let term = link.get_i64().expect("expected Integer argument");
///         total += term;
///     }
///
///     // Write the return value to the link.
///     link.put_i64(total).unwrap();
/// }
///
/// export_wstp![total_args_i64(&mut Link)];
/// # }
/// ```
///
/// ```wolfram
/// LibraryFunctionLoad["...", "total_args_i64", LinkObject, LinkObject]
/// ```
#[macro_export]
macro_rules! export_wstp {
    ($vis:vis $name:ident($($argc:ty),*) as $exported:ident) => {
        $vis mod $name {
            // Ensure that types imported into the enclosing parent module can be used in
            // the expansion of $argc. Always `Link` or `Vec<Expr>` at the moment.
            use super::*;

            #[no_mangle]
            pub unsafe extern "C" fn $exported(
                lib: $crate::sys::WolframLibraryData,
                raw_link: $crate::wstp::sys::WSLINK,
            ) -> std::os::raw::c_uint {
                // Cast away the unique `fn(...) {some_name}` function type to get the
                // generic `fn(...)` type.
                // The number of `$argc` is required for type inference of the variadic
                // `fn(..) -> _` type to work. See constraint 2a.
                let func: fn($($argc),*) -> _ = super::$name;

                // TODO: Why does this code work:
                //   let func: fn(&mut _) = super::$name;
                // but this does not:
                //   let func: fn(_) = super::$name;

                $crate::macro_utils::call_wstp_wolfram_library_function(
                    lib,
                    raw_link,
                    func
                )
            }

            // Register this exported function.
            $crate::inventory::submit! {
                $crate::macro_utils::LibraryLinkFunction::Wstp { name: stringify!($exported) }
            }
        }
    };

    // Convert export![name(..)] to export![name(..) as name].
    ($vis:vis $name:ident($($argc:ty),*)) => {
        $crate::export_wstp![$vis $name($($argc),*) as $name];
    };

    ($($vis:vis $name:ident($($argc:ty),*) $(as $exported:ident)?);* $(;)?) => {
        $(
            $crate::export_wstp![$vis $name($($argc),*) $(as $exported)?];
        )*
    };
}

// TODO: Allow any type which implements FromExpr in wrapper parameter lists?

/// Generate and export a "loader" function, which returns an Association containing the
/// names and loaded forms of all functions exported by this library.
///
/// All functions exported by the [`export!`] and [`export_wstp!`] macros will
/// automatically be included in the Association returned by this function.
///
/// # Syntax
///
/// Generate and export an automatic loader function.
///
/// ```
/// # use wolfram_library_link::generate_loader;
/// generate_loader![load_my_library];
/// ```
///
/// # Example
///
/// The following Rust program exports three primary functions via LibraryLink:
///
/// * `add2`
/// * `flat_total_i64`
/// * `time_since_epoch`
///
/// These functions are exported from the library using the [`export!`] and
/// [`export_wstp!`] macros. This makes them loadable using
/// [`LibraryFunctionLoad`][ref/LibraryFunctionLoad]<sub>WL</sub>.
///
///
/// ```
/// # mod scope {
/// use wolfram_library_link::{self as wll, NumericArray, expr::{Expr, Symbol, Number}};
///
/// wll::generate_loader![load_my_library_functions];
///
/// wll::export![
///     add2(_, _);
///     flat_total_i64(_);
/// ];
/// wll::export_wstp![time_since_epoch(_)];
///
/// fn add2(x: i64, y: i64) -> i64 {
///     x + y
/// }
///
/// fn flat_total_i64(list: &NumericArray<i64>) -> i64 {
///     list.as_slice().into_iter().sum()
/// }
///
/// fn time_since_epoch(args: Vec<Expr>) -> Expr {
///     use std::time::{SystemTime, UNIX_EPOCH};
///
///     assert!(args.len() == 0, "expected no arguments, got {}", args.len());
///
///     let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
///
///     Expr::normal(Symbol::new("System`Quantity").unwrap(), vec![
///         Expr::number(Number::real(duration.as_secs_f64())),
///         Expr::string("Seconds")
///     ])
/// }
/// # }
/// ```
///
/// However, writing out the correct invocations of
/// [`LibraryFunctionLoad`][ref/LibraryFunctionLoad] can be tedious and error prone.
/// `generate_loader!` provides an easier alternative way to load the functions
/// exported by this library.
///
/// In addition to the three previously mentioned functions, this library also exports a
/// fourth function, called `load_my_library_functions`.
///
/// Instead of writing three separate `LibraryFunctionLoad` calls, one for each exported
/// function, you can instead load the single `load_my_library_functions` function, which,
/// when called, will automatically load the other three functions exported by this
/// library:
///
/// ```wolfram
/// library = "example_library";
///
/// loadFunctions = LibraryFunctionLoad[library, "load_my_library_functions", LinkObject, LinkObject];
///
/// functions = loadFunctions[library];
/// ```
///
/// The `functions` variable will be an association, with roughly the following content:
///
/// ```wolfram
/// <|
///     "add2" -> LibraryFunction["example_library", "add2", {Integer, Integer}, Integer],
///     "flat_total_i64" -> LibraryFunction[
///         "example_library",
///         "flat_total_i64",
///         {{LibraryDataType[NumericArray, "Integer64"], "Constant"}},
///         Integer
///     ],
///     "time_since_epoch" -> LibraryFunction["example_library", "time_since_epoch", LinkObject]
/// |>
/// ```
///
/// As shown above, the `load_my_library_functions` function generated by
/// `generate_loader!` has automatically mapped the Rust paramater and return types onto
/// the appropriate Wolfram LibraryLink types.
///
/// Functions from the library can be called by applying arguments to the appropriate
/// value from the `functions` association:
///
/// ```wolfram
/// (* Returns 12 *)
/// functions["add2"][4, 8]
///
/// (* Returns 6 *)
/// functions["flat_total_i64"][NumericArray[{1, 2, 3}, "Integer64"]]
///
/// (* Returns Quantity[seconds_, "Seconds"], containing the current number of seconds
///    since the Unix epoch time. *)
/// functions["time_since_epoch"][]
/// ```
///
/// [ref/LibraryFunctionLoad]: https://reference.wolfram.com/language/ref/LibraryFunctionLoad.html
#[macro_export]
macro_rules! generate_loader {
    ($name:ident) => {
        // TODO: Use this anonymous `const` trick in export! and export_wstp! too.
        const _: () = {
            #[no_mangle]
            pub unsafe extern "C" fn $name(
                lib: $crate::sys::WolframLibraryData,
                raw_link: $crate::wstp::sys::WSLINK,
            ) -> std::os::raw::c_uint {
                $crate::macro_utils::load_library_functions_impl(lib, raw_link)
            }
        };
    };
}
