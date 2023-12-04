use anyhow::Result;
use chrono::{NaiveDate, NaiveDateTime};
use clap::Parser;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();

    let file = File::open(options.logfile).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut next_line_number = 0;

    let mut context = Context::new();

    while let Some(input_line) = lines.next_line().await? {
        let line_number = next_line_number;
        next_line_number += 1;

        let line = parse::line(line_number, &input_line, &mut context);

        let line = match line {
            Some(line) => line,
            None => continue,
        };

        println!("{line:?}");

        match line {
            Line::ThisRuntimeId(_) => (),
            Line::ReceivedRootNode(line) => {
                let key = (line.that_label.clone(), line.common_prefix.label.clone());
                let value = context.from_to_root_nodes.entry(key).or_default();
                value.push(line.common_prefix.line_number);
            }
            Line::ReceivedInnerNode(line) => {
                let key = (line.that_label.clone(), line.common_prefix.label.clone());
                let value = context.from_to_inner_nodes.entry(key).or_default();
                value.push(line.common_prefix.line_number);
            }
            Line::ReceivedLeafNode(line) => {
                let key = (line.that_label.clone(), line.common_prefix.label.clone());
                let value = context.from_to_leaf_nodes.entry(key).or_default();
                value.push(line.common_prefix.line_number);
            }
            Line::ReceivedBlock(line) => {
                let key = (line.that_label.clone(), line.common_prefix.label.clone());
                let value = context.from_to_blocks.entry(key).or_default();
                value.push(line.common_prefix.line_number);
            }
        }
    }

    println!("RootNodes");
    for ((from, to), lines) in &context.from_to_root_nodes {
        println!("  {to:?} <- {from:?}: {}    {:?}", lines.len(), lines);
    }
    println!("InnerNodes");
    for ((from, to), lines) in &context.from_to_inner_nodes {
        println!("  {to:?} <- {from:?}: {}    {:?}", lines.len(), lines);
    }
    println!("LeafNodes");
    for ((from, to), lines) in &context.from_to_leaf_nodes {
        println!("  {to:?} <- {from:?}: {}     {:?}", lines.len(), lines);
    }
    println!("Blocks");
    for ((from, to), lines) in &context.from_to_blocks {
        println!("  {to:?} <- {from:?}: {}     {:?}", lines.len(), lines);
    }

    Ok(())
}

struct Context {
    runtime_id_to_label: HashMap<String, String>,
    from_to_root_nodes: BTreeMap<(String, String), Vec<u32>>,
    from_to_inner_nodes: BTreeMap<(String, String), Vec<u32>>,
    from_to_leaf_nodes: BTreeMap<(String, String), Vec<u32>>,
    from_to_blocks: BTreeMap<(String, String), Vec<u32>>,
}

impl Context {
    fn new() -> Self {
        Self {
            runtime_id_to_label: Default::default(),
            from_to_root_nodes: Default::default(),
            from_to_inner_nodes: Default::default(),
            from_to_leaf_nodes: Default::default(),
            from_to_blocks: Default::default(),
        }
    }
}

#[derive(Debug)]
enum Line {
    ThisRuntimeId(ThisRuntimeIdLine),
    ReceivedRootNode(ReceivedRootNodeLine),
    ReceivedInnerNode(ReceivedInnerNodeLine),
    ReceivedLeafNode(ReceivedLeafNodeLine),
    ReceivedBlock(ReceivedBlockLine),
}

#[derive(Debug)]
struct ThisRuntimeIdLine {
    common_prefix: CommonPrefix,
    this_runtime_id: String,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ReceivedRootNodeLine {
    common_prefix: CommonPrefix,
    that_label: String,
    hash: String,
    exchange_id: u32,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ReceivedInnerNodeLine {
    common_prefix: CommonPrefix,
    that_label: String,
    exchange_id: u32,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ReceivedLeafNodeLine {
    common_prefix: CommonPrefix,
    that_label: String,
    exchange_id: u32,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ReceivedBlockLine {
    common_prefix: CommonPrefix,
    that_label: String,
    block_id: String,
}

/// Utility to analyze network communication from swarm logs.
///
#[derive(Debug, Parser)]
struct Options {
    /// Logfile to analyze
    #[arg(short = 'l', long)]
    logfile: PathBuf,
}

#[allow(dead_code)]
#[derive(Debug)]
struct CommonPrefix {
    line_number: u32,
    label: String,
    date: NaiveDateTime,
    log_level: String,
}

mod parse {
    use super::*;

    pub(super) fn line(line_number: u32, mut input: &str, context: &mut Context) -> Option<Line> {
        if let Some(line) = this_runtime_id_line(line_number, &mut input) {
            context.runtime_id_to_label.insert(
                line.this_runtime_id.clone(),
                line.common_prefix.label.clone(),
            );
            return Some(Line::ThisRuntimeId(line));
        }
        if let Some(line) = received_root_node_line(line_number, &mut input, context) {
            return Some(Line::ReceivedRootNode(line));
        }
        if let Some(line) = received_inner_node_line(line_number, &mut input, context) {
            return Some(Line::ReceivedInnerNode(line));
        }
        if let Some(line) = received_leaf_node_line(line_number, &mut input, context) {
            return Some(Line::ReceivedLeafNode(line));
        }
        if let Some(line) = received_block_line(line_number, &mut input, context) {
            return Some(Line::ReceivedBlock(line));
        }
        None
    }

    fn this_runtime_id_line(line_number: u32, s: &mut &str) -> Option<ThisRuntimeIdLine> {
        let common_prefix = common_prefix(line_number, s)?;
        find_string("this_runtime_id=", s)?;
        let this_runtime_id = alphanumeric_string(s)?.into();
        Some(ThisRuntimeIdLine {
            common_prefix,
            this_runtime_id,
        })
    }

    fn received_root_node_line(
        line_number: u32,
        s: &mut &str,
        context: &Context,
    ) -> Option<ReceivedRootNodeLine> {
        let common_prefix = common_prefix(line_number, s)?;
        find_string("Received root node", s)?;
        find_string(" hash=", s)?;
        let hash = alphanumeric_string(s)?.into();
        find_string("DebugResponse { exchange_id: ", s)?;
        let exchange_id = digits(s)?.parse().ok()?;
        find_string("message_broker{", s)?;
        let that_runtime_id = alphanumeric_string(s)?;
        Some(ReceivedRootNodeLine {
            common_prefix,
            that_label: context
                .runtime_id_to_label
                .get(that_runtime_id)
                .unwrap()
                .clone(),
            hash,
            exchange_id,
        })
    }

    fn received_inner_node_line(
        line_number: u32,
        s: &mut &str,
        context: &Context,
    ) -> Option<ReceivedInnerNodeLine> {
        let common_prefix = common_prefix(line_number, s)?;
        find_string("Received ", s)?;
        digits(s)?;
        char('/', s)?;
        digits(s)?;
        string(" inner nodes:", s)?;
        find_string("DebugResponse { exchange_id: ", s)?;
        let exchange_id = digits(s)?.parse().ok()?;
        find_string("message_broker{", s)?;
        let that_runtime_id = alphanumeric_string(s)?;
        Some(ReceivedInnerNodeLine {
            common_prefix,
            that_label: context
                .runtime_id_to_label
                .get(that_runtime_id)
                .unwrap()
                .clone(),
            exchange_id,
        })
    }

    fn received_leaf_node_line(
        line_number: u32,
        s: &mut &str,
        context: &Context,
    ) -> Option<ReceivedLeafNodeLine> {
        let common_prefix = common_prefix(line_number, s)?;
        find_string("Received ", s)?;
        digits(s)?;
        char('/', s)?;
        digits(s)?;
        string(" leaf nodes:", s)?;
        find_string("DebugResponse { exchange_id: ", s)?;
        let exchange_id = digits(s)?.parse().ok()?;
        find_string("message_broker{", s)?;
        let that_runtime_id = alphanumeric_string(s)?;
        Some(ReceivedLeafNodeLine {
            common_prefix,
            that_label: context
                .runtime_id_to_label
                .get(that_runtime_id)
                .unwrap()
                .clone(),
            exchange_id,
        })
    }

    fn received_block_line(
        line_number: u32,
        s: &mut &str,
        context: &Context,
    ) -> Option<ReceivedBlockLine> {
        let common_prefix = common_prefix(line_number, s)?;
        find_string("Received block :: handle_block{id=", s)?;
        let block_id = alphanumeric_string(s)?.into();
        find_string("message_broker{", s)?;
        let that_runtime_id = alphanumeric_string(s)?;
        Some(ReceivedBlockLine {
            common_prefix,
            that_label: context
                .runtime_id_to_label
                .get(that_runtime_id)
                .unwrap()
                .clone(),
            block_id,
        })
    }

    fn common_prefix(line_number: u32, s: &mut &str) -> Option<CommonPrefix> {
        let label = label(s)?;
        white_space(s)?;
        let date = date(s)?;
        white_space(s)?;
        let log_level = alphanumeric_string(s)?.into();
        white_space(s)?;
        Some(CommonPrefix {
            line_number,
            label,
            date,
            log_level,
        })
    }

    fn date(s: &mut &str) -> Option<NaiveDateTime> {
        let year = num_i32(s)?;
        char('-', s)?;
        let month = num_u32(s)?;
        char('-', s)?;
        let day = num_u32(s)?;
        char('T', s)?;
        let hour = num_u32(s)?;
        char(':', s)?;
        let minute = num_u32(s)?;
        char(':', s)?;
        let second = num_u32(s)?;
        char('.', s)?;
        let decimal = digits(s)?;
        char('Z', s)?;

        let mut micro = 0;
        let mut coef = 100000;

        for d in decimal.chars().take(6) {
            micro += d.to_digit(10 /* RADIX */)? * coef;
            coef /= 10;
        }

        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_micro_opt(hour, minute, second, micro)
    }

    pub(super) fn label(s: &mut &str) -> Option<String> {
        char('[', s)?;
        let l = alphanumeric_string(s)?;
        white_space(s).unwrap_or(());
        char(']', s)?;
        Some(l.into())
    }

    pub(super) fn num_u32(s: &mut &str) -> Option<u32> {
        digits(s)?.parse().ok()
    }

    pub(super) fn num_i32(s: &mut &str) -> Option<i32> {
        digits(s)?.parse().ok()
    }

    pub(super) fn digits<'a>(s: &mut &'a str) -> Option<&'a str> {
        take_while(
            |s| {
                s.chars()
                    .next()
                    .map(|c| if c.is_ascii_digit() { 1 } else { 0 })
                    .unwrap_or(0)
            },
            s,
        )
    }

    pub(super) fn is_alphanumeric(c: char) -> bool {
        c.is_ascii_digit() || c.is_ascii_uppercase() || c.is_ascii_lowercase()
    }

    pub(super) fn alphanumeric_char(s: &mut &str) -> Option<char> {
        let c = match s.chars().next() {
            Some(c) => c,
            None => return None,
        };
        if is_alphanumeric(c) {
            *s = &s[c.len_utf8()..];
            return Some(c);
        }
        None
    }

    pub(super) fn alphanumeric_string<'a>(s: &mut &'a str) -> Option<&'a str> {
        take_one_or_more(|s| alphanumeric_char(s).map(|c| c.len_utf8()), s)
    }

    pub(super) fn string(prefix: &str, s: &mut &str) -> Option<()> {
        if s.starts_with(prefix) {
            *s = &s[prefix.len()..];
            Some(())
        } else {
            None
        }
    }

    pub(super) fn char(c: char, s: &mut &str) -> Option<usize> {
        if s.starts_with(c) {
            let l = c.len_utf8();
            *s = &s[l..];
            Some(l)
        } else {
            None
        }
    }

    fn find_string(prefix: &str, s: &mut &str) -> Option<()> {
        loop {
            if string(prefix, s).is_some() {
                return Some(());
            }

            any_char(s)?;
        }
    }

    pub(super) fn any_char(s: &mut &str) -> Option<usize> {
        let c = s.chars().next();
        if let Some(c) = c {
            let l = c.len_utf8();
            *s = &s[l..];
            Some(l)
        } else {
            None
        }
    }

    pub(super) fn count<F>(f: F, s: &mut &str) -> Option<usize>
    where
        F: Fn(&mut &str) -> Option<usize>,
    {
        let mut count = None;
        while let Some(n) = f(s) {
            match &mut count {
                Some(c) => *c += n,
                None => count = Some(n),
            }
        }
        count
    }

    pub(super) fn take_one_or_more<'a, F>(f: F, s: &mut &'a str) -> Option<&'a str>
    where
        F: Fn(&mut &str) -> Option<usize>,
    {
        let mut s2 = *s;
        let c = match count(f, &mut s2) {
            Some(c) => c,
            None => return None,
        };
        let (parsed, rest) = s.split_at(c);
        *s = rest;
        Some(parsed)
    }

    pub(super) fn take_while<'a, F>(f: F, s: &mut &'a str) -> Option<&'a str>
    where
        F: Fn(&str) -> usize,
    {
        let mut rest = *s;
        loop {
            let n = f(rest);
            if n == 0 {
                break;
            }
            rest = &rest[n..];
        }

        let ret = &s[0..(s.len() - rest.len())];
        *s = rest;
        Some(ret)
    }

    pub(super) fn white_space(s: &mut &str) -> Option<()> {
        count(|s| char(' ', s), s).map(|_| ())
    }
}
