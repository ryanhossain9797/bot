use std::sync::LazyLock;

use regex::Regex;

static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<@!?@?([A-Za-z0-9_.]{1,64})>").expect("static mention regex is valid")
});

#[derive(Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Id(u64),
    Handle(&'a str),
}

pub fn mention_segments(text: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut last = 0;
    for cap in MENTION.captures_iter(text) {
        let whole = cap.get(0).expect("capture 0 is the whole match");
        let target = cap.get(1).expect("the pattern has one group").as_str();
        if whole.start() > last {
            segments.push(Segment::Text(&text[last..whole.start()]));
        }
        segments.push(match target.parse() {
            Ok(id) => Segment::Id(id),
            Err(_) => Segment::Handle(target),
        });
        last = whole.end();
    }
    if last < text.len() {
        segments.push(Segment::Text(&text[last..]));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_split_into_ids_and_handles() {
        assert_eq!(
            mention_segments("hi <@123>, <@!45> and <@bob_k> or <@@ann.b>; <@ not> <@a b>"),
            vec![
                Segment::Text("hi "),
                Segment::Id(123),
                Segment::Text(", "),
                Segment::Id(45),
                Segment::Text(" and "),
                Segment::Handle("bob_k"),
                Segment::Text(" or "),
                Segment::Handle("ann.b"),
                Segment::Text("; <@ not> <@a b>"),
            ]
        );
        assert!(mention_segments("").is_empty());
    }
}
