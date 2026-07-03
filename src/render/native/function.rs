//! PDF Function evaluation (ISO 32000-1:2008 §7.10 "Functions"), used by
//! [`super::colorspace`] to evaluate a Separation/DeviceN colour space's
//! `/TintTransform` (§8.6.6.3 "Separation Colour Spaces", §8.6.6.4 "DeviceN
//! Colour Spaces").
//!
//! # Scope
//!
//! The task asked specifically for Function Types 0 (Sampled) and 4
//! (PostScript calculator) -- "secukupnya" (enough to evaluate a tint
//! transform). This module implements those two, plus Types 2 (Exponential
//! Interpolation) and 3 (Stitching), because in real-world PDFs those two
//! are actually the *more* common encoding for a simple Separation tint
//! transform (Type 2 alone, or a Type 3 stitching several Type 2 pieces
//! together) and are direct, simple compositions of the same machinery --
//! not a new gap surface, just more of the same spec section. Any function
//! of a type not listed here (there are only these four in ISO 32000-1) --
//! or one that is malformed enough that this module cannot parse it at all
//! -- makes [`parse_function`] return `None`; the caller
//! ([`super::colorspace::resolve_color_space`]) then marks the whole colour
//! space as [`super::colorspace::ColorSpace::Unsupported`] rather than
//! guessing, and callers of *that* skip painting with a recorded
//! [`super::error::RenderWarning::UnsupportedColorSpace`] -- never a panic,
//! never a silently-wrong colour.
//!
//! # Type 4 (PostScript calculator) operator coverage
//!
//! Implements the arithmetic (`add sub mul div idiv mod neg abs ceiling
//! floor round truncate sqrt sin cos atan exp ln log cvi cvr`), stack
//! (`pop exch dup copy index roll`), boolean/relational (`eq ne gt ge lt le
//! and or not xor bitshift true false`) and conditional (`if ifelse`)
//! operators from ISO 32000-1 Table 42 -- i.e. the complete operator set
//! Table 42 actually lists (Type 4 functions have **no** looping construct
//! at all per the spec, so there is no "unsupported loop" gap to speak of).
//! An operator token this evaluator doesn't recognise (a hand-edited/
//! corrupt program) aborts evaluation of that function call with `None`
//! rather than guessing.
//!
//! # Untrusted input handling
//!
//! - Sampled-function dimensionality is capped
//!   ([`MAX_SAMPLED_DIMENSIONS`]) so a crafted `/Size` array cannot force
//!   `2^m` corner interpolation to blow up memory/CPU.
//! - Stitching-function fan-out is capped ([`MAX_STITCH_FUNCTIONS`]).
//! - Function nesting (a Type 3 stitching function's sub-functions, which
//!   are themselves parsed recursively) is capped
//!   ([`MAX_FUNCTION_PARSE_DEPTH`]) both while parsing and while
//!   evaluating, since a stitching function's sub-function is *data*, not
//!   an indirect reference this module could form a true reference cycle
//!   through -- but deep nesting could otherwise still exhaust the call
//!   stack.
//! - The PostScript calculator interpreter bounds total executed operators
//!   ([`MAX_PS_STEPS`]) and operand-stack depth ([`MAX_PS_STACK`]).

use std::rc::Rc;

use crate::object::{Object, PdfArray, PdfDictionary};

use super::bits::BitReader;

/// Hard cap on Sampled-function (Type 0) input dimensionality. `2^m`
/// hypercube corners are visited per evaluation; 8 keeps that at a
/// trivially cheap 256 in the worst case while comfortably covering every
/// real-world Separation (`m=1`) or DeviceN (`m` = a handful of colorants)
/// use this crate has seen.
const MAX_SAMPLED_DIMENSIONS: usize = 8;

/// Hard cap on a Type 3 stitching function's `/Functions` fan-out.
const MAX_STITCH_FUNCTIONS: usize = 256;

/// Hard cap on how deeply [`parse_function`]/[`PdfFunction::eval`] may
/// recurse into nested (Type 3 stitching) sub-functions.
const MAX_FUNCTION_PARSE_DEPTH: usize = 16;

/// Hard cap on total PostScript-calculator operators executed by one
/// [`PdfFunction::eval`] call (across nested `if`/`ifelse` bodies).
const MAX_PS_STEPS: usize = 100_000;

/// Hard cap on the PostScript-calculator operand stack depth.
const MAX_PS_STACK: usize = 200;

/// Hard cap on parenthesis/brace nesting depth while parsing a Type 4
/// program (defends the recursive-descent parser against a stack overflow
/// on a maliciously deep `{{{{...}}}}` nesting).
const MAX_PS_PARSE_DEPTH: usize = 64;

/// A parsed, evaluatable PDF function object (ISO 32000-1 §7.10).
#[derive(Debug)]
pub(super) enum PdfFunction {
    Sampled(SampledFunction),
    Exponential(ExponentialFunction),
    Stitching(StitchingFunction),
    PostScript(PostScriptFunction),
}

impl PdfFunction {
    /// Evaluates the function at `input`, returning the output component
    /// vector, or `None` if evaluation could not complete (malformed
    /// program/data, or the nesting/step/stack bounds above were
    /// exceeded). Never panics on adversarial `input` or function data.
    ///
    /// Note on "eval": this only walks this module's own tiny, sandboxed
    /// numeric interpreter (arithmetic/stack/relational/conditional
    /// operators over a bounded operand stack, per §7.10.5's fixed
    /// operator table -- see the [module docs](self)). It never executes
    /// host code, shells out, or interprets any language beyond that
    /// closed operator set, and every loop/recursion/stack path here is
    /// depth- or step-bounded against adversarial PDF input.
    pub(super) fn eval(&self, input: &[f64]) -> Option<Vec<f64>> {
        self.eval_inner(input, 0)
    }

    fn eval_inner(&self, input: &[f64], depth: usize) -> Option<Vec<f64>> {
        if depth > MAX_FUNCTION_PARSE_DEPTH {
            return None;
        }
        match self {
            PdfFunction::Sampled(f) => f.eval(input),
            PdfFunction::Exponential(f) => f.eval(input),
            PdfFunction::Stitching(f) => f.eval(input, depth),
            PdfFunction::PostScript(f) => f.eval(input),
        }
    }
}

/// Linear interpolation of `x` from `[x0, x1]` into `[y0, y1]` (ISO 32000-1
/// 7.10.2's `Interpolate` pseudocode). Falls back to `y0` for a degenerate
/// (zero-width) input range rather than dividing by zero.
fn interpolate(x: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
    if (x1 - x0).abs() < f64::EPSILON {
        return y0;
    }
    y0 + (x - x0) * (y1 - y0) / (x1 - x0)
}

fn sanitize(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

fn as_f64(o: &Object) -> Option<f64> {
    o.as_real().filter(|v| v.is_finite())
}

fn numbers(arr: &PdfArray) -> Vec<f64> {
    arr.iter().filter_map(as_f64).collect()
}

fn pairs(arr: &PdfArray) -> Vec<(f64, f64)> {
    let flat = numbers(arr);
    flat.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

fn object_dict(obj: &Object) -> Option<&PdfDictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dictionary),
        _ => None,
    }
}

fn object_data(obj: &Object) -> Option<&[u8]> {
    match obj {
        Object::Stream(s) => Some(&s.data),
        _ => None,
    }
}

/// Parses a PDF function object (a `Dictionary` for Types 2/3, a `Stream`
/// for Types 0/4 -- ISO 32000-1 §7.10) into an evaluatable [`PdfFunction`].
/// Returns `None` for anything this module cannot parse: a missing
/// required entry, a `/FunctionType` other than 0/2/3/4, nesting deeper
/// than [`MAX_FUNCTION_PARSE_DEPTH`], or a Type 4 program this module's
/// small PostScript-calculator parser rejects.
pub(super) fn parse_function(obj: &Object, depth: usize) -> Option<PdfFunction> {
    if depth > MAX_FUNCTION_PARSE_DEPTH {
        return None;
    }
    let dict = object_dict(obj)?;
    let ftype = dict.get("FunctionType").and_then(Object::as_integer)?;
    let domain = pairs(dict.get("Domain")?.as_array()?);
    if domain.is_empty() {
        return None;
    }
    let range = dict
        .get("Range")
        .and_then(Object::as_array)
        .map(pairs);

    match ftype {
        0 => {
            let data = object_data(obj)?;
            let size: Vec<u32> = dict
                .get("Size")?
                .as_array()?
                .iter()
                .filter_map(Object::as_integer)
                .map(|v| v.max(0) as u32)
                .collect();
            if size.is_empty() || size.len() != domain.len() || size.len() > MAX_SAMPLED_DIMENSIONS {
                return None;
            }
            let bps = dict.get("BitsPerSample").and_then(Object::as_integer)?;
            if !(1..=32).contains(&bps) {
                return None;
            }
            let bps = bps as u32;
            let range = range?;
            let n_out = range.len();
            if n_out == 0 {
                return None;
            }
            let encode = dict
                .get("Encode")
                .and_then(Object::as_array)
                .map(pairs)
                .unwrap_or_else(|| size.iter().map(|&s| (0.0, s.max(1) as f64 - 1.0)).collect());
            let decode = dict
                .get("Decode")
                .and_then(Object::as_array)
                .map(pairs)
                .unwrap_or_else(|| range.clone());
            if encode.len() != size.len() || decode.len() != n_out {
                return None;
            }
            Some(PdfFunction::Sampled(SampledFunction {
                domain,
                range,
                size,
                bps,
                encode,
                decode,
                data: data.to_vec(),
                n_out,
            }))
        }
        2 => {
            let c0 = dict
                .get("C0")
                .and_then(Object::as_array)
                .map(numbers)
                .unwrap_or_else(|| vec![0.0]);
            let c1 = dict
                .get("C1")
                .and_then(Object::as_array)
                .map(numbers)
                .unwrap_or_else(|| vec![1.0]);
            let n = dict.get("N").and_then(as_f64).unwrap_or(1.0);
            Some(PdfFunction::Exponential(ExponentialFunction {
                domain: domain[0],
                c0,
                c1,
                n,
                range,
            }))
        }
        3 => {
            let funcs_arr = dict.get("Functions")?.as_array()?;
            if funcs_arr.is_empty() || funcs_arr.len() > MAX_STITCH_FUNCTIONS {
                return None;
            }
            let mut functions = Vec::with_capacity(funcs_arr.len());
            for f in funcs_arr.iter() {
                functions.push(parse_function(f, depth + 1)?);
            }
            let bounds = dict
                .get("Bounds")
                .and_then(Object::as_array)
                .map(numbers)
                .unwrap_or_default();
            if bounds.len() != functions.len() - 1 {
                return None;
            }
            let encode = dict
                .get("Encode")
                .and_then(Object::as_array)
                .map(pairs)
                .unwrap_or_default();
            if encode.len() != functions.len() {
                return None;
            }
            Some(PdfFunction::Stitching(StitchingFunction {
                domain: domain[0],
                functions,
                bounds,
                encode,
                range,
            }))
        }
        4 => {
            let data = object_data(obj)?;
            let program = parse_postscript_program(data)?;
            let range = range?;
            if range.is_empty() {
                return None;
            }
            Some(PdfFunction::PostScript(PostScriptFunction {
                domain,
                range,
                program,
            }))
        }
        _ => None,
    }
}

/// Type 0: Sampled Function (ISO 32000-1 §7.10.2). General `m`-input,
/// `n`-output multilinear interpolation over a rectangular sample grid.
/// Higher-order (`/Order 3`, cubic spline) interpolation is not
/// implemented -- every function is evaluated with (order-1) multilinear
/// interpolation regardless of a declared `/Order 3`, which is the same
/// simplification most non-print-production PDF renderers make (the
/// visual difference is subtle -- smooth vs. very-slightly-less-smooth
/// gradation of a tint ramp -- not a structural gap like JBIG2/JPX).
#[derive(Debug)]
pub(super) struct SampledFunction {
    domain: Vec<(f64, f64)>,
    range: Vec<(f64, f64)>,
    size: Vec<u32>,
    bps: u32,
    encode: Vec<(f64, f64)>,
    decode: Vec<(f64, f64)>,
    data: Vec<u8>,
    n_out: usize,
}

impl SampledFunction {
    fn eval(&self, input: &[f64]) -> Option<Vec<f64>> {
        let m = self.domain.len();
        if input.len() < m || m == 0 {
            return None;
        }

        let mut lo = vec![0u32; m];
        let mut frac = vec![0f64; m];
        for i in 0..m {
            let (d0, d1) = self.domain[i];
            let x = input[i].clamp(d0.min(d1), d0.max(d1));
            let (e0, e1) = self.encode[i];
            let size_i = self.size[i].max(1);
            let e = interpolate(x, d0, d1, e0, e1).clamp(0.0, (size_i - 1) as f64);
            let lo_i = e.floor() as u32;
            lo[i] = lo_i.min(size_i.saturating_sub(1));
            frac[i] = sanitize(e - lo_i as f64);
        }

        let n = self.n_out;
        let mut acc = vec![0f64; n];
        let corners = 1usize << m;
        for mask in 0..corners {
            let mut weight = 1.0f64;
            let mut idx_coord = vec![0u32; m];
            for (i, coord) in idx_coord.iter_mut().enumerate() {
                let use_hi = (mask >> i) & 1 == 1;
                let size_i = self.size[i].max(1);
                *coord = if use_hi {
                    (lo[i] + 1).min(size_i - 1)
                } else {
                    lo[i]
                };
                weight *= if use_hi { frac[i] } else { 1.0 - frac[i] };
            }
            if weight == 0.0 {
                continue;
            }

            let mut flat: u64 = 0;
            let mut mult: u64 = 1;
            for (i, &coord) in idx_coord.iter().enumerate() {
                flat += u64::from(coord) * mult;
                mult *= u64::from(self.size[i].max(1));
            }

            let bit_offset = flat
                .saturating_mul(n as u64)
                .saturating_mul(u64::from(self.bps));
            let mut reader = BitReader::new(&self.data);
            reader.seek_bit(bit_offset);
            let max_raw = ((1u64 << self.bps) - 1) as f64;
            for (j, out) in acc.iter_mut().enumerate() {
                let raw = reader.read_bits(self.bps);
                let (dec0, dec1) = self.decode[j];
                let val = interpolate(f64::from(raw), 0.0, max_raw, dec0, dec1);
                *out += weight * val;
            }
        }

        for (j, out) in acc.iter_mut().enumerate() {
            let (r0, r1) = self.range[j];
            *out = sanitize(*out).clamp(r0.min(r1), r0.max(r1));
        }
        Some(acc)
    }
}

/// Type 2: Exponential Interpolation Function (ISO 32000-1 §7.10.3):
/// `y_j = C0_j + x^N * (C1_j - C0_j)`.
#[derive(Debug)]
pub(super) struct ExponentialFunction {
    domain: (f64, f64),
    c0: Vec<f64>,
    c1: Vec<f64>,
    n: f64,
    range: Option<Vec<(f64, f64)>>,
}

impl ExponentialFunction {
    fn eval(&self, input: &[f64]) -> Option<Vec<f64>> {
        let (d0, d1) = self.domain;
        let x = input.first().copied()?.clamp(d0.min(d1), d0.max(d1));
        let n_out = self.c0.len().max(self.c1.len()).max(1);
        let xp = if self.n == 1.0 { x } else { x.powf(self.n) };
        let mut out: Vec<f64> = (0..n_out)
            .map(|i| {
                let c0 = self.c0.get(i).copied().unwrap_or(0.0);
                let c1 = self.c1.get(i).copied().unwrap_or(1.0);
                sanitize(c0 + xp * (c1 - c0))
            })
            .collect();
        if let Some(range) = &self.range {
            for (i, (r0, r1)) in range.iter().enumerate() {
                if let Some(v) = out.get_mut(i) {
                    *v = v.clamp(r0.min(*r1), r0.max(*r1));
                }
            }
        }
        Some(out)
    }
}

/// Type 3: Stitching Function (ISO 32000-1 §7.10.4): partitions a
/// single-input domain into `k` sub-domains, each delegated to one of `k`
/// sub-functions.
#[derive(Debug)]
pub(super) struct StitchingFunction {
    domain: (f64, f64),
    functions: Vec<PdfFunction>,
    bounds: Vec<f64>,
    encode: Vec<(f64, f64)>,
    range: Option<Vec<(f64, f64)>>,
}

impl StitchingFunction {
    fn eval(&self, input: &[f64], depth: usize) -> Option<Vec<f64>> {
        let (d0, d1) = self.domain;
        let x = input.first().copied()?.clamp(d0.min(d1), d0.max(d1));

        let mut idx = 0usize;
        while idx < self.bounds.len() && x >= self.bounds[idx] {
            idx += 1;
        }
        let low = if idx == 0 { d0 } else { self.bounds[idx - 1] };
        let high = if idx == self.bounds.len() { d1 } else { self.bounds[idx] };
        let (e0, e1) = *self.encode.get(idx)?;
        let e = interpolate(x, low, high, e0, e1);

        let mut out = self.functions.get(idx)?.eval_inner(&[e], depth + 1)?;
        if let Some(range) = &self.range {
            for (i, (r0, r1)) in range.iter().enumerate() {
                if let Some(v) = out.get_mut(i) {
                    *v = v.clamp(r0.min(*r1), r0.max(*r1));
                }
            }
        }
        Some(out)
    }
}

/// Type 4: PostScript Calculator Function (ISO 32000-1 §7.10.5). See the
/// [module docs](self) for the exact operator subset implemented.
#[derive(Debug)]
pub(super) struct PostScriptFunction {
    domain: Vec<(f64, f64)>,
    range: Vec<(f64, f64)>,
    program: Vec<PsToken>,
}

impl PostScriptFunction {
    fn eval(&self, input: &[f64]) -> Option<Vec<f64>> {
        if input.len() < self.domain.len() {
            return None;
        }
        let mut state = PsState {
            stack: Vec::new(),
            steps: 0,
        };
        for (i, &(d0, d1)) in self.domain.iter().enumerate() {
            let x = input[i].clamp(d0.min(d1), d0.max(d1));
            push_num(&mut state, x)?;
        }
        run(&self.program, &mut state)?;

        let n = self.range.len();
        if state.stack.len() < n {
            return None;
        }
        let start = state.stack.len() - n;
        let mut out = Vec::with_capacity(n);
        for v in &state.stack[start..] {
            match v {
                PsValue::Num(x) => out.push(*x),
                PsValue::Proc(_) => return None,
            }
        }
        for (i, (r0, r1)) in self.range.iter().enumerate() {
            out[i] = out[i].clamp(r0.min(*r1), r0.max(*r1));
        }
        Some(out)
    }
}

/// One token of a parsed Type 4 program: a numeric literal, an operator
/// keyword, or a `{ ... }` procedure block (pushed as a value by `if`/
/// `ifelse`'s two preceding operands, then executed -- not immediately --
/// by the conditional operator itself).
#[derive(Debug, Clone)]
enum PsToken {
    Number(f64),
    Op(String),
    Block(Rc<[PsToken]>),
}

/// Splits `src` into whitespace/`{`/`}`-delimited raw tokens.
fn lex(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in src.chars() {
        if ch == '{' || ch == '}' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(ch.to_string());
        } else if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_block(tokens: &[String], pos: &mut usize, depth: usize) -> Option<Vec<PsToken>> {
    if depth > MAX_PS_PARSE_DEPTH {
        return None;
    }
    let mut out = Vec::new();
    while *pos < tokens.len() {
        let tok = &tokens[*pos];
        if tok == "}" {
            *pos += 1;
            return Some(out);
        }
        if tok == "{" {
            *pos += 1;
            let inner = parse_block(tokens, pos, depth + 1)?;
            out.push(PsToken::Block(Rc::from(inner.into_boxed_slice())));
            continue;
        }
        if let Ok(n) = tok.parse::<f64>() {
            out.push(PsToken::Number(n));
        } else {
            out.push(PsToken::Op(tok.clone()));
        }
        *pos += 1;
    }
    Some(out)
}

/// Parses a Type 4 program's raw stream bytes (ISO 32000-1 §7.10.5.1),
/// tolerating (and stripping) the conventional outer `{ ... }` wrapper.
fn parse_postscript_program(src: &[u8]) -> Option<Vec<PsToken>> {
    let text = std::str::from_utf8(src).ok()?;
    let tokens = lex(text);
    let mut pos = 0;
    if tokens.first().map(String::as_str) == Some("{") {
        pos = 1;
    }
    parse_block(&tokens, &mut pos, 0)
}

#[derive(Debug, Clone)]
enum PsValue {
    Num(f64),
    Proc(Rc<[PsToken]>),
}

struct PsState {
    stack: Vec<PsValue>,
    steps: usize,
}

fn push_value(state: &mut PsState, v: PsValue) -> Option<()> {
    if state.stack.len() >= MAX_PS_STACK {
        return None;
    }
    state.stack.push(v);
    Some(())
}

fn push_num(state: &mut PsState, n: f64) -> Option<()> {
    push_value(state, PsValue::Num(sanitize(n)))
}

fn pop_num(state: &mut PsState) -> Option<f64> {
    match state.stack.pop()? {
        PsValue::Num(n) => Some(n),
        PsValue::Proc(_) => None,
    }
}

fn pop_proc(state: &mut PsState) -> Option<Rc<[PsToken]>> {
    match state.stack.pop()? {
        PsValue::Proc(p) => Some(p),
        PsValue::Num(_) => None,
    }
}

fn push_bool(state: &mut PsState, b: bool) -> Option<()> {
    push_num(state, if b { 1.0 } else { 0.0 })
}

fn run(tokens: &[PsToken], state: &mut PsState) -> Option<()> {
    for tok in tokens {
        state.steps += 1;
        if state.steps > MAX_PS_STEPS {
            return None;
        }
        match tok {
            PsToken::Number(n) => push_num(state, *n)?,
            PsToken::Block(b) => push_value(state, PsValue::Proc(b.clone()))?,
            PsToken::Op(op) => exec_op(op, state)?,
        }
    }
    Some(())
}

/// Executes one PostScript-calculator operator. See the [module docs](self)
/// for exactly which operators from ISO 32000-1 Table 42 are implemented.
/// Returns `None` (aborting the whole function evaluation) for an unknown
/// operator, a stack-type mismatch (e.g. `add` with a procedure operand),
/// or a stack underflow -- never a panic.
fn exec_op(op: &str, state: &mut PsState) -> Option<()> {
    match op {
        "add" => bin(state, |a, b| a + b),
        "sub" => bin(state, |a, b| a - b),
        "mul" => bin(state, |a, b| a * b),
        "div" => bin(state, |a, b| if b != 0.0 { a / b } else { 0.0 }),
        "idiv" => bin(state, |a, b| {
            let bi = b as i64;
            if bi != 0 {
                (a as i64 / bi) as f64
            } else {
                0.0
            }
        }),
        "mod" => bin(state, |a, b| {
            let bi = b as i64;
            if bi != 0 {
                (a as i64 % bi) as f64
            } else {
                0.0
            }
        }),
        "atan" => bin(state, |num, den| {
            let mut deg = num.atan2(den).to_degrees();
            if deg < 0.0 {
                deg += 360.0;
            }
            deg
        }),
        "exp" => bin(state, |base, e| base.powf(e)),
        "neg" => un(state, |a| -a),
        "abs" => un(state, |a| a.abs()),
        "sqrt" => un(state, |a| a.max(0.0).sqrt()),
        "sin" => un(state, |a| a.to_radians().sin()),
        "cos" => un(state, |a| a.to_radians().cos()),
        "ln" => un(state, |a| if a > 0.0 { a.ln() } else { 0.0 }),
        "log" => un(state, |a| if a > 0.0 { a.log10() } else { 0.0 }),
        "ceiling" => un(state, f64::ceil),
        "floor" => un(state, f64::floor),
        "round" => un(state, f64::round),
        "truncate" | "cvi" => un(state, f64::trunc),
        "cvr" => Some(()),
        "dup" => {
            let v = state.stack.last().cloned()?;
            push_value(state, v)
        }
        "pop" => state.stack.pop().map(|_| ()),
        "exch" => {
            let len = state.stack.len();
            if len < 2 {
                return None;
            }
            state.stack.swap(len - 1, len - 2);
            Some(())
        }
        "copy" => {
            let n = pop_num(state)?;
            if !(0.0..=(MAX_PS_STACK as f64)).contains(&n) {
                return None;
            }
            let n = n as usize;
            if n > state.stack.len() || state.stack.len() + n > MAX_PS_STACK {
                return None;
            }
            let start = state.stack.len() - n;
            let slice: Vec<PsValue> = state.stack[start..].to_vec();
            state.stack.extend(slice);
            Some(())
        }
        "index" => {
            let n = pop_num(state)?;
            if n < 0.0 {
                return None;
            }
            let n = n as usize;
            let len = state.stack.len();
            if n >= len {
                return None;
            }
            let v = state.stack[len - 1 - n].clone();
            push_value(state, v)
        }
        "roll" => {
            let j = pop_num(state)? as i64;
            let n = pop_num(state)?;
            if n < 0.0 {
                return None;
            }
            let n = n as usize;
            let len = state.stack.len();
            if n > len {
                return None;
            }
            if n > 0 {
                let j_norm = (((j % n as i64) + n as i64) % n as i64) as usize;
                state.stack[len - n..].rotate_right(j_norm);
            }
            Some(())
        }
        "eq" => bin_bool(state, |a, b| a == b),
        "ne" => bin_bool(state, |a, b| a != b),
        "gt" => bin_bool(state, |a, b| a > b),
        "ge" => bin_bool(state, |a, b| a >= b),
        "lt" => bin_bool(state, |a, b| a < b),
        "le" => bin_bool(state, |a, b| a <= b),
        "and" => bin(state, |a, b| ((a as i64) & (b as i64)) as f64),
        "or" => bin(state, |a, b| ((a as i64) | (b as i64)) as f64),
        "xor" => bin(state, |a, b| ((a as i64) ^ (b as i64)) as f64),
        "not" => un(state, |a| {
            if a == 0.0 {
                1.0
            } else if a == 1.0 {
                0.0
            } else {
                !(a as i64) as f64
            }
        }),
        "bitshift" => bin(state, |a, shift| {
            let a = a as i64;
            let shift = shift as i64;
            let r = if shift >= 0 {
                a.checked_shl(shift.min(63) as u32).unwrap_or(0)
            } else {
                a >> (-shift).min(63)
            };
            r as f64
        }),
        "true" => push_bool(state, true),
        "false" => push_bool(state, false),
        "if" => {
            let proc_ = pop_proc(state)?;
            let cond = pop_num(state)?;
            if cond != 0.0 {
                run(&proc_, state)?;
            }
            Some(())
        }
        "ifelse" => {
            let proc2 = pop_proc(state)?;
            let proc1 = pop_proc(state)?;
            let cond = pop_num(state)?;
            if cond != 0.0 {
                run(&proc1, state)?;
            } else {
                run(&proc2, state)?;
            }
            Some(())
        }
        _ => None,
    }
}

/// `a b <op>`: pops `b` then `a` (so the first-pushed operand is `a`),
/// applies `f(a, b)`, pushes the result.
fn bin(state: &mut PsState, f: impl FnOnce(f64, f64) -> f64) -> Option<()> {
    let b = pop_num(state)?;
    let a = pop_num(state)?;
    push_num(state, f(a, b))
}

fn bin_bool(state: &mut PsState, f: impl FnOnce(f64, f64) -> bool) -> Option<()> {
    let b = pop_num(state)?;
    let a = pop_num(state)?;
    push_bool(state, f(a, b))
}

fn un(state: &mut PsState, f: impl FnOnce(f64) -> f64) -> Option<()> {
    let a = pop_num(state)?;
    push_num(state, f(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfStream;

    fn arr(v: Vec<Object>) -> Object {
        Object::Array(PdfArray::from_objects(v))
    }

    #[test]
    fn exponential_identity_at_n1() {
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(2));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("C0", arr(vec![Object::Real(0.0)]));
        dict.set("C1", arr(vec![Object::Real(1.0)]));
        dict.set("N", Object::Real(1.0));
        let f = parse_function(&Object::Dictionary(dict), 0).unwrap();
        let out = f.eval(&[0.5]).unwrap();
        assert!((out[0] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn exponential_inverts_via_c0_c1() {
        // Common Separation tint transform pattern: 1-tint -> darker as
        // tint increases (C0=1, C1=0).
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(2));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("C0", arr(vec![Object::Real(1.0)]));
        dict.set("C1", arr(vec![Object::Real(0.0)]));
        dict.set("N", Object::Real(1.0));
        let f = parse_function(&Object::Dictionary(dict), 0).unwrap();
        assert!((f.eval(&[0.0]).unwrap()[0] - 1.0).abs() < 1e-9);
        assert!((f.eval(&[1.0]).unwrap()[0] - 0.0).abs() < 1e-9);
    }

    #[test]
    fn postscript_simple_arithmetic() {
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(4));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Range", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        let program = b"{ 1 exch sub }"; // 1-x
        let stream = Object::Stream(PdfStream::with_dictionary(dict, program.to_vec()));
        let f = parse_function(&stream, 0).unwrap();
        let out = f.eval(&[0.25]).unwrap();
        assert!((out[0] - 0.75).abs() < 1e-9);
    }

    #[test]
    fn postscript_ifelse() {
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(4));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Range", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        // x > 0.5 -> 1, else 0.
        let program = b"{ 0.5 gt { 1 } { 0 } ifelse }";
        let stream = Object::Stream(PdfStream::with_dictionary(dict, program.to_vec()));
        let f = parse_function(&stream, 0).unwrap();
        assert_eq!(f.eval(&[0.9]).unwrap()[0], 1.0);
        assert_eq!(f.eval(&[0.1]).unwrap()[0], 0.0);
    }

    #[test]
    fn postscript_unknown_operator_fails_closed_not_panicking() {
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(4));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Range", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        let program = b"{ frobnicate }";
        let stream = Object::Stream(PdfStream::with_dictionary(dict, program.to_vec()));
        let f = parse_function(&stream, 0).unwrap();
        assert!(f.eval(&[0.5]).is_none());
    }

    #[test]
    fn postscript_stack_underflow_fails_closed_not_panicking() {
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(4));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Range", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        let program = b"{ add add add add }"; // way more pops than pushes
        let stream = Object::Stream(PdfStream::with_dictionary(dict, program.to_vec()));
        let f = parse_function(&stream, 0).unwrap();
        assert!(f.eval(&[0.5]).is_none());
    }

    #[test]
    fn stitching_dispatches_to_correct_subfunction() {
        // Two Type 2 halves: [0,0.5) -> constant 0, [0.5,1] -> constant 1.
        let mut sub0 = PdfDictionary::new();
        sub0.set("FunctionType", Object::Integer(2));
        sub0.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        sub0.set("C0", arr(vec![Object::Real(0.0)]));
        sub0.set("C1", arr(vec![Object::Real(0.0)]));
        sub0.set("N", Object::Real(1.0));

        let mut sub1 = PdfDictionary::new();
        sub1.set("FunctionType", Object::Integer(2));
        sub1.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        sub1.set("C0", arr(vec![Object::Real(1.0)]));
        sub1.set("C1", arr(vec![Object::Real(1.0)]));
        sub1.set("N", Object::Real(1.0));

        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(3));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set(
            "Functions",
            arr(vec![Object::Dictionary(sub0), Object::Dictionary(sub1)]),
        );
        dict.set("Bounds", arr(vec![Object::Real(0.5)]));
        dict.set(
            "Encode",
            arr(vec![
                Object::Real(0.0),
                Object::Real(1.0),
                Object::Real(0.0),
                Object::Real(1.0),
            ]),
        );
        let f = parse_function(&Object::Dictionary(dict), 0).unwrap();
        assert_eq!(f.eval(&[0.1]).unwrap()[0], 0.0);
        assert_eq!(f.eval(&[0.9]).unwrap()[0], 1.0);
    }

    #[test]
    fn sampled_1d_linear_ramp() {
        // 1-input, 1-output, 2-sample table: 0 -> 0.0, 1(max) -> 1.0 at
        // 8 bits per sample (0 and 255).
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(0));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Range", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Size", arr(vec![Object::Integer(2)]));
        dict.set("BitsPerSample", Object::Integer(8));
        let data = vec![0u8, 255u8];
        let stream = Object::Stream(PdfStream::with_dictionary(dict, data));
        let f = parse_function(&stream, 0).unwrap();
        let out = f.eval(&[0.5]).unwrap();
        assert!((out[0] - 0.5).abs() < 0.01, "got {}", out[0]);
        assert!((f.eval(&[0.0]).unwrap()[0] - 0.0).abs() < 1e-9);
        assert!((f.eval(&[1.0]).unwrap()[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn malformed_function_type_returns_none_not_panic() {
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(99));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        assert!(parse_function(&Object::Dictionary(dict), 0).is_none());
    }

    #[test]
    fn oversized_stitch_fan_out_is_rejected() {
        let mut subs = Vec::new();
        let mut bounds = Vec::new();
        let mut encode = Vec::new();
        for i in 0..(MAX_STITCH_FUNCTIONS + 1) {
            let mut s = PdfDictionary::new();
            s.set("FunctionType", Object::Integer(2));
            s.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
            subs.push(Object::Dictionary(s));
            if i > 0 {
                bounds.push(Object::Real(i as f64 / (MAX_STITCH_FUNCTIONS + 1) as f64));
            }
            encode.push(Object::Real(0.0));
            encode.push(Object::Real(1.0));
        }
        let mut dict = PdfDictionary::new();
        dict.set("FunctionType", Object::Integer(3));
        dict.set("Domain", arr(vec![Object::Real(0.0), Object::Real(1.0)]));
        dict.set("Functions", arr(subs));
        dict.set("Bounds", arr(bounds));
        dict.set("Encode", arr(encode));
        assert!(parse_function(&Object::Dictionary(dict), 0).is_none());
    }

    #[test]
    fn deeply_nested_braces_do_not_stack_overflow_parser() {
        let mut program = String::new();
        for _ in 0..(MAX_PS_PARSE_DEPTH + 10) {
            program.push('{');
        }
        for _ in 0..(MAX_PS_PARSE_DEPTH + 10) {
            program.push('}');
        }
        assert!(parse_postscript_program(program.as_bytes()).is_none());
    }
}
