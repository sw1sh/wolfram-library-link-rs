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
//!   [`Expr`][struct@wl_expr::Expr] and the [`#[wolfram_library_function]`][wlf] macro.
//! * Asynchronous events handled by the Wolfram Language, generated using a background
//!   thread spawned via [`AsyncTaskObject`].
//!
//! #### Related Links
//!
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
//! [[building your dynamic library]], and loading the function into the Wolfram Language
//! using [`LibraryFunctionLoad`][library-function-load]:
//!
//! ```wolfram
//! func = LibraryFunctionLoad["library_name", "square", {Integer}, Integer];
//!
//! func[5]   (* Returns 25 *)
//! ```
//!
//! ## Show backtrace when a panic occurs
//!
//! Functions wrapped using [`wolfram_library_function`][wlf] will automatically catch any
//! Rust panic's which occur in the wrapped code, and return a [`Failure`][failure] object
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
//! [wlf]: attr.wolfram_library_function.html
//! [library-link-guide]: https://reference.wolfram.com/language/guide/LibraryLink.html
//! [library-function-load]: https://reference.wolfram.com/language/ref/LibraryFunctionLoad.html
//! [failure]: https://reference.wolfram.com/language/ref/Failure.html
//! [cargo-features]: https://doc.rust-lang.org/cargo/reference/features.html

#![cfg_attr(feature = "nightly", feature(panic_info_message))]
#![warn(missing_docs)]

mod args;
mod async_tasks;
/// This module is *semver exempt*. This is not intended to be part of the public API of
/// wolfram-library-link.
///
/// Utility for catching panics, capturing a backtrace, and extracting the panic
/// message.
#[doc(hidden)]
pub mod catch_panic;
mod data_store;
mod image;
mod library_data;
/// This module is *semver exempt*. This is not intended to be part of the public API of
/// wolfram-library-link.
///
/// Utilities used by code generated by the [`#[wolfram_library_function]`][wlf] macro.
///
/// [wlf]: attr.wolfram_library_function.html
#[doc(hidden)]
pub mod macro_utils;
mod numeric_array;
pub mod rtl;


use std::sync::Mutex;

use once_cell::sync::Lazy;

use wl_expr::{Expr, ExprKind};
use wl_symbol_table as sym;
use wolfram_library_link_sys::mint;
use wstp::Link;


pub use wolfram_library_link_sys as sys;
pub use wstp;

pub use self::{
    args::{FromArg, IntoArg, NativeFunction},
    async_tasks::AsyncTaskObject,
    data_store::DataStore,
    image::{ColorSpace, Image, ImageData, ImageType, Pixel, UninitImage},
    library_data::{get_library_data, initialize, WolframLibraryData},
    numeric_array::{
        NumericArray, NumericArrayConvertMethod, NumericArrayDataType, NumericArrayKind,
        NumericArrayType, UninitNumericArray,
    },
};

pub(crate) use self::library_data::assert_main_thread;

/// Attribute to generate a [LibraryLink][library-link]-compatible wrapper around a Rust
/// function.
///
/// The wrapper function generated by this macro must be loaded using
/// [`LibraryFunctionLoad`][library-function-load], with [`LinkObject`][link-object] as
/// the argument and return value types.
///
/// A function written like:
///
/// ```
/// use wl_expr::Expr;
/// use wolfram_library_link::{self as wll, wolfram_library_function};
///
/// #[wolfram_library_function]
/// pub fn say_hello(args: Vec<Expr>) -> Expr {
///     for arg in args {
///         wll::evaluate(&Expr! { Print["Hello ", 'arg] });
///     }
///
///     Expr::null()
/// }
/// ```
///
/// can be loaded in the Wolfram Language by evaluating:
///
/// ```wolfram
/// LibraryFunctionLoad[
///     "/path/to/target/debug/libmy_crate.dylib",
///     "say_hello_wrapper",
///     LinkObject,
///     LinkObject
/// ]
/// ```
///
/// ## Options
///
/// #### Generated wrapper name
///
/// By default, the generated wrapper function will be the name of the function the
/// attribute it applied to with the fragment `_wrapper` appended. For example, the
/// function `say_hello` has a wrapper named `say_hello_wrapper`.
///
/// This can be controlled via the `name` option of `wolfram_library_function`, which sets
/// the name of generated Wolfram library function:
///
/// ```
/// # use wl_expr::Expr;
/// # use wolfram_library_link::wolfram_library_function;
/// #
/// #[wolfram_library_function(name = "WL_greet")]
/// pub fn say_hello(args: Vec<Expr>) -> Expr {
///     // ...
/// #   Expr::null()
/// }
/// ```
///
/// The `LibraryFunctionLoad` invocation should change to:
///
/// ```wolfram
/// LibraryFunctionLoad[
///     "/path/to/target/debug/libmy_crate.dylib"
///     "WL_greet",
///     LinkObject,
///     LinkObject
/// ]
/// ```
///
///
/// [library-link]: https://reference.wolfram.com/language/guide/LibraryLink.html
/// [library-function-load]: https://reference.wolfram.com/language/ref/LibraryFunctionLoad.html
/// [link-object]: https://reference.wolfram.com/language/ref/LinkObject.html
#[doc(inline)]
pub use wolfram_library_function_macro::wolfram_library_function;

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
            .put_expr(&Expr! { EvaluatePacket['expr] })
            .map_err(|e| e.to_string())?;

        let _: () = process_wstp_link(link)?;

        let return_packet: Expr = link.get_expr().map_err(|e| e.to_string())?;

        let returned_expr = match return_packet.kind() {
            ExprKind::Normal(normal) => {
                debug_assert!(normal.has_head(&*sym::ReturnPacket));
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
/// Within Rust code reached through a `#[wolfram_library_function]` wrapper,
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
/// [`NativeFunction`] must be implemented by the functions
/// exported by this macro.
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
/// Export a native function with a single argument:
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::{export, NumericArray};
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
/// Export a native function with multiple arguments:
///
/// ```
/// fn reverse_string(string: String) -> String {
///     string.chars().rev().collect()
/// }
/// ```
///
/// ### Numeric arrays
///
/// Export a native function with a [`NumericArray`] argument:
///
/// ```
/// # mod scope {
/// # use wolfram_library_link::{export, NumericArray};
///
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
/// The following table describes the relationship between Rust types that implement
/// [`FromArg`] and the compatible Wolfram *LibraryLink* function parameter type(s).
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
// trait, which is implemented for all suitable `Fn(..) -> _` types.
//
// Constraint 1b is unable to be met completely by the current implementation due to
// limitations with Rust's coercion from `fn(A, B, ..) -> C` to `Fn(A, B, ..) -> C`. The
// coercion requires that the number of parameters (`foo(_, _)`) be made explicit, even
// if their types can be elided. If eliding the number of Fn(..) arguments were permitted,
// `export![foo]` could work.
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
                // The number of `$argc` is required for type inference of the variadic
                // `&dyn Fn(..) -> _` type to work. See constraint 2a.
                let func: &dyn Fn($($argc),*) -> _ = &super::$name;

                $crate::macro_utils::call_native_wolfram_library_function(
                    lib,
                    args,
                    argc,
                    res,
                    func
                )
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

// TODO: Allow any type which implements FromExpr in wrapper parameter lists?
