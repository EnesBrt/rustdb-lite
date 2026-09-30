//! Aggregate input filtering, stable ordering, DISTINCT and accumulation.
use super::*;
use crate::sql::sort::{compare_keys, sort_by};

struct Input {
    value: Value,
    separator: Option<Value>,
    keys: Vec<Value>,
}

impl Eval<'_> {
    /// SQLite's min/max steps also select the row supplying bare columns. A
    /// filtered-out step and a DISTINCT duplicate have different effects on
    /// that selection, especially before any non-NULL value has been seen.
    pub fn aggregate_representative(
        &mut self,
        extrema: &[&Expr],
        rows: &[Vec<Value>],
        queries: &mut dyn Subqueries,
    ) -> Result<Option<usize>> {
        let all_filtered = extrema.iter().all(|e| {
            matches!(
                &e.kind,
                ExprKind::Call {
                    filter: Some(_),
                    ..
                }
            )
        });
        let mut best = alloc::vec![Value::Null; extrema.len()];
        let mut seen: Vec<Vec<Value>> = alloc::vec![Vec::new(); extrema.len()];
        let mut bytes = 0;
        let mut skip = false;
        let mut selected = None;
        for (row_index, row) in rows.iter().enumerate() {
            for (i, expr) in extrema.iter().enumerate() {
                self.fuel.spend()?;
                let ExprKind::Call {
                    name,
                    args,
                    distinct,
                    filter,
                    ..
                } = &expr.kind
                else {
                    return Err(Error::Corrupt("extremum expression"));
                };
                if let Some(filter) = filter {
                    if all_filtered {
                        skip = row_index != 0;
                    }
                    if scalar::truth(&self.eval_with(filter, row, None, queries)?)? != Some(true) {
                        continue;
                    }
                }
                let value = self.eval_with(&args[0], row, None, queries)?;
                if *distinct {
                    let mut duplicate = false;
                    for previous in &seen[i] {
                        self.fuel.spend()?;
                        if scalar::compare_encoded(
                            &value,
                            previous,
                            collation(&args[0]),
                            self.encoding,
                        )? == Ordering::Equal
                        {
                            duplicate = true;
                            break;
                        }
                    }
                    if duplicate {
                        continue;
                    }
                    charge(&mut bytes, &value, self.limits)?;
                    seen[i].push(value.clone());
                }
                let cmp =
                    scalar::compare_encoded(&value, &best[i], collation(&args[0]), self.encoding)?;
                skip = !scalar::null(&best[i])
                    && (scalar::null(&value)
                        || !(name == "min" && cmp == Ordering::Less
                            || name == "max" && cmp == Ordering::Greater));
                if !skip {
                    charge(&mut bytes, &value, self.limits)?;
                    best[i] = value;
                }
            }
            if !skip {
                selected = Some(row_index);
            }
        }
        Ok(selected)
    }

    pub(super) fn aggregate(
        &mut self,
        expr: &Expr,
        rows: &[Vec<Value>],
        queries: &mut dyn Subqueries,
    ) -> Result<Value> {
        let ExprKind::Call {
            name,
            args,
            star,
            distinct,
            order,
            filter,
        } = &expr.kind
        else {
            return Err(Error::Corrupt("aggregate expression"));
        };
        // Native min/max validates but does not evaluate its ORDER BY terms.
        let order = if name == "min" || name == "max" {
            &[][..]
        } else {
            order.as_slice()
        };
        // A DISTINCT key identical to the sole ORDER BY key uses one unique
        // sorting table in SQLite. Equal keys replace the previous payload.
        let unique_order =
            *distinct && order.len() == 1 && args.len() == 1 && order[0].expr == args[0];
        let mut inputs: Vec<Input> = Vec::new();
        let mut value_bytes = 0usize;
        for row in rows {
            self.fuel.spend()?;
            if let Some(filter) = filter {
                if scalar::truth(&self.eval_with(filter, row, None, queries)?)? != Some(true) {
                    continue;
                }
            }
            let mut input_bytes = core::mem::size_of::<Input>();
            let mut keys = Vec::new();
            for term in order {
                let key = self.eval_with(&term.expr, row, None, queries)?;
                charge(&mut input_bytes, &key, self.limits)?;
                keys.push(key);
            }
            let v = if *star || (name == "count" && args.is_empty()) {
                Value::Integer(1)
            } else {
                self.eval_with(&args[0], row, None, queries)?
            };
            charge(&mut input_bytes, &v, self.limits)?;
            let separator = if args.len() > 1 {
                Some(self.eval_with(&args[1], row, None, queries)?)
            } else {
                None
            };
            if let Some(separator) = &separator {
                charge(&mut input_bytes, separator, self.limits)?;
            }
            if scalar::null(&v) {
                continue;
            }
            let mut duplicate = None;
            if *distinct {
                for (i, previous) in inputs.iter().enumerate() {
                    self.fuel.spend()?;
                    if scalar::compare_encoded(
                        &v,
                        &previous.value,
                        collation(&args[0]),
                        self.encoding,
                    )? == Ordering::Equal
                    {
                        duplicate = Some(i);
                        break;
                    }
                }
                if duplicate.is_some() && !unique_order {
                    continue;
                }
            }
            value_bytes = value_bytes
                .checked_add(input_bytes)
                .ok_or(Error::Limit("aggregate bytes"))?;
            if value_bytes > self.limits.max_database_bytes {
                return Err(Error::Limit("aggregate bytes"));
            }
            let input = Input {
                value: v,
                separator,
                keys,
            };
            if let Some(i) = duplicate {
                inputs[i] = input;
            } else {
                inputs.push(input);
            }
        }
        if !order.is_empty() {
            let encoding = self.encoding;
            inputs = sort_by(inputs, self.fuel, |a, b| {
                compare_keys(&a.keys, &b.keys, order, encoding)
            })?;
        }
        let values: Vec<_> = inputs
            .into_iter()
            .map(|input| (input.value, input.separator))
            .collect();
        if name == "count" {
            return Ok(Value::Integer(values.len() as i64));
        }
        if name == "group_concat" || name == "string_agg" {
            if values.is_empty() {
                return Ok(Value::Null);
            }
            let mut out = Vec::new();
            for (i, (value, separator)) in values.iter().enumerate() {
                let separator = if let Some(s) = separator {
                    scalar::text_bytes(s)?
                } else {
                    alloc::vec![b',']
                };
                let value = scalar::text_bytes(value)?;
                let extra = value
                    .len()
                    .checked_add(if i == 0 { 0 } else { separator.len() })
                    .and_then(|n| n.checked_add(out.len()))
                    .ok_or(Error::Limit("SQL value size"))?;
                if extra > self.limits.max_value_bytes {
                    return Err(Error::Limit("SQL value size"));
                }
                if i != 0 {
                    out.extend_from_slice(&separator);
                }
                out.extend_from_slice(&value);
            }
            return Ok(Value::Text(Text {
                bytes: out,
                encoding: Encoding::Utf8,
            }));
        }
        if name == "min" || name == "max" {
            let mut result = Value::Null;
            for (v, _) in values {
                let cmp = scalar::compare_encoded(&v, &result, collation(&args[0]), self.encoding)?;
                if scalar::null(&result)
                    || (name == "min" && cmp == Ordering::Less)
                    || (name == "max" && cmp == Ordering::Greater)
                {
                    result = v;
                }
            }
            return Ok(result);
        }
        if values.is_empty() {
            return Ok(if name == "total" {
                Value::Real(0.0)
            } else {
                Value::Null
            });
        }
        let mut integer_sum = 0i64;
        let mut approximate = false;
        let mut overflow = false;
        let mut sum = 0.0f64;
        let mut correction = 0.0f64;
        for (v, _) in &values {
            // Aggregate numeric_type differs from arithmetic coercion: fully
            // numeric text may remain an integer, but BLOBs do not acquire one.
            let typed = scalar::numeric_type(v)?;
            if !approximate {
                if let Value::Integer(n) = typed {
                    if let Some(next) = integer_sum.checked_add(n) {
                        integer_sum = next;
                        continue;
                    }
                    overflow = true;
                }
                approximate = true;
                let small = integer_sum % 16384;
                sum = (integer_sum - small) as f64;
                correction = small as f64;
            }
            if let Value::Integer(n) = typed {
                if n.unsigned_abs() >= 4503599627370496 {
                    let small = n % 16384;
                    compensated_add(&mut sum, &mut correction, (n - small) as f64);
                    compensated_add(&mut sum, &mut correction, small as f64);
                } else {
                    compensated_add(&mut sum, &mut correction, n as f64);
                }
            } else {
                overflow = false;
                compensated_add(
                    &mut sum,
                    &mut correction,
                    scalar::float(&scalar::numeric(&typed)?),
                );
            }
        }
        if name == "sum" {
            if overflow {
                return Err(error("integer overflow"));
            }
            if !approximate {
                return Ok(Value::Integer(integer_sum));
            }
        }
        let result = if !approximate {
            integer_sum as f64
        } else if correction.is_finite() {
            sum + correction
        } else {
            sum
        };
        Ok(scalar::real(if name == "avg" {
            result / values.len() as f64
        } else {
            result
        }))
    }
}

fn charge(bytes: &mut usize, value: &Value, limits: SqlLimits) -> Result<()> {
    *bytes = bytes
        .checked_add(scalar::size(value) + core::mem::size_of::<Value>())
        .ok_or(Error::Limit("aggregate bytes"))?;
    if *bytes > limits.max_database_bytes {
        return Err(Error::Limit("aggregate bytes"));
    }
    Ok(())
}
