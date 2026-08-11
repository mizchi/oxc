// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Compiler diagnostics, built directly on [`oxc_diagnostics`].
//!
//! Passes construct [`OxcDiagnostic`]s eagerly via [`ErrorCategory::diagnostic`],
//! whose deterministic `[ReactCompiler] <Category>: ` message prefix lets
//! consumers recover the category for control flow (Invariant/Config checks,
//! panic-threshold severity) without a parallel data model.
//!
//! Errors "thrown" by a pass (TS: exceptions escaping a pass) propagate as a
//! single `Err(OxcDiagnostic)`; errors accumulated on the Environment and
//! returned at the end of the pipeline travel as
//! [`Diagnostics`](oxc_diagnostics::Diagnostics).

use std::borrow::Cow;

use oxc_diagnostics::{OxcDiagnostic, Severity};
use oxc_span::Span;

use crate::options::PanicThreshold;

/// Error categories matching the TS `ErrorCategory` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Hooks,
    CapitalizedCalls,
    StaticComponents,
    UseMemo,
    VoidUseMemo,
    PreserveManualMemo,
    MemoDependencies,
    IncompatibleLibrary,
    Immutability,
    Globals,
    Refs,
    EffectExhaustiveDependencies,
    EffectSetState,
    EffectDerivationsOfState,
    ErrorBoundaries,
    Purity,
    RenderSetState,
    Invariant,
    Todo,
    Syntax,
    UnsupportedSyntax,
    Config,
    Gating,
    Suppression,
}

impl ErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hooks => "Hooks",
            Self::CapitalizedCalls => "CapitalizedCalls",
            Self::StaticComponents => "StaticComponents",
            Self::UseMemo => "UseMemo",
            Self::VoidUseMemo => "VoidUseMemo",
            Self::PreserveManualMemo => "PreserveManualMemo",
            Self::MemoDependencies => "MemoDependencies",
            Self::IncompatibleLibrary => "IncompatibleLibrary",
            Self::Immutability => "Immutability",
            Self::Globals => "Globals",
            Self::Refs => "Refs",
            Self::EffectExhaustiveDependencies => "EffectExhaustiveDependencies",
            Self::EffectSetState => "EffectSetState",
            Self::EffectDerivationsOfState => "EffectDerivationsOfState",
            Self::ErrorBoundaries => "ErrorBoundaries",
            Self::Purity => "Purity",
            Self::RenderSetState => "RenderSetState",
            Self::Invariant => "Invariant",
            Self::Todo => "Todo",
            Self::Syntax => "Syntax",
            Self::UnsupportedSyntax => "UnsupportedSyntax",
            Self::Config => "Config",
            Self::Gating => "Gating",
            Self::Suppression => "Suppression",
        }
    }

    /// Displayed severity, matching the TS compiler's `getRuleForCategory()`.
    /// `PreserveManualMemo` displays as an error but does not count towards
    /// `panicThreshold: critical_errors` (see [`has_critical_errors`]).
    const fn severity(self) -> Severity {
        match self {
            Self::IncompatibleLibrary | Self::UnsupportedSyntax | Self::Todo => Severity::Warning,
            _ => Severity::Error,
        }
    }

    /// Build a diagnostic for this category: `[ReactCompiler] <Category>: <reason>`.
    /// Attach spans with `.with_label(span)` (plain underline) or
    /// `.with_label(span.label(text))`, and a description with `.with_help(..)`.
    pub fn diagnostic(self, reason: impl AsRef<str>) -> OxcDiagnostic {
        let message = format!("[ReactCompiler] {}: {}", self.as_str(), reason.as_ref());
        match self.severity() {
            Severity::Error => OxcDiagnostic::error(message),
            _ => OxcDiagnostic::warn(message),
        }
    }

    /// The category whose [`Self::as_str`] is `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "Hooks" => Self::Hooks,
            "CapitalizedCalls" => Self::CapitalizedCalls,
            "StaticComponents" => Self::StaticComponents,
            "UseMemo" => Self::UseMemo,
            "VoidUseMemo" => Self::VoidUseMemo,
            "PreserveManualMemo" => Self::PreserveManualMemo,
            "MemoDependencies" => Self::MemoDependencies,
            "IncompatibleLibrary" => Self::IncompatibleLibrary,
            "Immutability" => Self::Immutability,
            "Globals" => Self::Globals,
            "Refs" => Self::Refs,
            "EffectExhaustiveDependencies" => Self::EffectExhaustiveDependencies,
            "EffectSetState" => Self::EffectSetState,
            "EffectDerivationsOfState" => Self::EffectDerivationsOfState,
            "ErrorBoundaries" => Self::ErrorBoundaries,
            "Purity" => Self::Purity,
            "RenderSetState" => Self::RenderSetState,
            "Invariant" => Self::Invariant,
            "Todo" => Self::Todo,
            "Syntax" => Self::Syntax,
            "UnsupportedSyntax" => Self::UnsupportedSyntax,
            "Config" => Self::Config,
            "Gating" => Self::Gating,
            "Suppression" => Self::Suppression,
            _ => return None,
        })
    }

    /// Whether `diagnostic` was built for this category via [`Self::diagnostic`],
    /// recovered from the deterministic message prefix.
    pub fn matches(self, diagnostic: &OxcDiagnostic) -> bool {
        Self::of(diagnostic) == Some(self.as_str())
    }

    /// The category segment of a message built by [`Self::diagnostic`].
    fn of(diagnostic: &OxcDiagnostic) -> Option<&str> {
        let rest = diagnostic.message.strip_prefix("[ReactCompiler] ")?;
        rest.split_once(": ").map(|(category, _)| category)
    }
}

/// A lint finding paired with its [`ErrorCategory`].
///
/// `diagnostic.message` is the bare reason — the internal
/// `[ReactCompiler] <Category>: ` prefix carried by compile-mode diagnostics
/// is already stripped.
#[derive(Debug, Clone)]
pub struct LintDiagnostic {
    pub category: ErrorCategory,
    pub diagnostic: OxcDiagnostic,
}

/// Split a diagnostic built by [`ErrorCategory::diagnostic`] into its category
/// and the bare-reason diagnostic.
pub(crate) fn categorize(mut diagnostic: OxcDiagnostic) -> LintDiagnostic {
    let parsed = diagnostic
        .message
        .strip_prefix("[ReactCompiler] ")
        .and_then(|rest| rest.split_once(": "))
        .and_then(|(name, reason)| {
            ErrorCategory::from_name(name).map(|category| (category, reason.to_string()))
        });
    match parsed {
        Some((category, reason)) => {
            diagnostic.message = Cow::Owned(reason);
            LintDiagnostic { category, diagnostic }
        }
        None => {
            // Only `log_error`'s mirror of upstream's test-only simulated
            // unknown exception (`Pipeline error: …`) legitimately escapes
            // the `<Category>: <reason>` scheme; treat it as an internal
            // error and keep everything after the shared prefix.
            debug_assert!(
                diagnostic.message.starts_with("[ReactCompiler] Pipeline error: "),
                "malformed React Compiler diagnostic message: {}",
                diagnostic.message
            );
            let stripped =
                diagnostic.message.strip_prefix("[ReactCompiler] ").map(ToString::to_string);
            if let Some(stripped) = stripped {
                diagnostic.message = Cow::Owned(stripped);
            }
            LintDiagnostic { category: ErrorCategory::Invariant, diagnostic }
        }
    }
}

/// Whether any diagnostic is an error at the TS compiler's *internal*
/// severity, which decides `panicThreshold: critical_errors`. Internal and
/// displayed severity agree except for `PreserveManualMemo`, which displays
/// as an error but is internally a warning (it must not trigger the panic
/// threshold).
pub fn has_critical_errors(diagnostics: &[OxcDiagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error && !ErrorCategory::PreserveManualMemo.matches(d))
}

/// Whether diagnostics should abort compilation for the configured panic threshold.
///
/// Config errors are always fatal, matching the upstream compiler.
pub fn should_panic(diagnostics: &[OxcDiagnostic], panic_threshold: PanicThreshold) -> bool {
    diagnostics.iter().any(|d| ErrorCategory::Config.matches(d))
        || match panic_threshold {
            PanicThreshold::AllErrors => true,
            PanicThreshold::CriticalErrors => has_critical_errors(diagnostics),
            PanicThreshold::None => false,
        }
}

/// Owned copy of a diagnostic for the log accumulator, labelling the enclosing
/// function (`fn_span`) when the diagnostic carries no location of its own.
#[cold]
pub fn with_fallback_label(diagnostic: &OxcDiagnostic, fn_span: Option<Span>) -> OxcDiagnostic {
    let diagnostic = diagnostic.clone();
    match fn_span {
        Some(span) if diagnostic.labels.is_empty() => diagnostic.with_label(span),
        _ => diagnostic,
    }
}
