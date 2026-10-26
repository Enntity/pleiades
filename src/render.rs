// SPDX-License-Identifier: AGPL-3.0-only
//! Rendering catalog arg templates with per-rank values.
use crate::catalog::ArgSpec;
use anyhow::{Result, bail};
use std::collections::BTreeMap;

pub type Vars = BTreeMap<String, String>;

/// Replace every `{name}` with vars[name]; an unknown placeholder is an error.
pub fn render(template: &str, vars: &Vars) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .map(|e| start + e)
            .ok_or_else(|| anyhow::anyhow!("unclosed placeholder in {template:?}"))?;
        let name = &rest[start + 1..end];
        match vars.get(name) {
            Some(v) => out.push_str(v),
            None => bail!("unknown placeholder {{{name}}} in {template:?}"),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Render an arg list: an optional flag/value renders nothing when the value
/// renders empty; everything else renders verbatim after substitution.
pub fn render_args(args: &[ArgSpec], vars: &Vars) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for spec in args {
        match spec {
            ArgSpec::Lit(s) => out.push(render(s, vars)?),
            ArgSpec::Opt {
                flag,
                value,
                optional,
            } => {
                let v = render(value, vars)?;
                if *optional && v.is_empty() {
                    continue;
                }
                out.push(render(flag, vars)?);
                out.push(v);
            }
        }
    }
    Ok(out)
}
