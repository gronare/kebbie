use super::Comment;

pub fn addresses_line(comment: &Comment) -> String {
    format!("Addresses {}'s comment on {}.", super::login(&comment.author), comment.short())
}

// The reader writes the subject and body; kebbie adds the Addresses line itself, last, so the fix is found by it once pushed.
pub fn fix_message(comment: &Comment) -> String {
    let line = addresses_line(comment);
    let reading = comment.reading.clone().unwrap_or_default();
    let subject = if reading.subject.is_empty() { format!("Address {}'s comment on {}", super::login(&comment.author), comment.short()) } else { reading.subject };
    let body: Vec<&str> = reading.body.lines().filter(|text| text.trim() != line).collect();
    let body = body.join("\n").trim().to_string();
    if body.is_empty() { format!("{subject}\n\n{line}") } else { format!("{subject}\n\n{body}\n\n{line}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{Kind, Status};
    use crate::comments::tests::comment;

    #[test]
    fn the_fix_message_ends_with_the_addresses_line_once() {
        let mut read = comment(1, Some(Kind::Holds), Status::Read);
        assert_eq!(fix_message(&read), "Count partial refunds in the refund total\n\nThe total counted refunds, not their amounts.\n\nAddresses johan's comment on 3f2a9c1.");
        read.reading.as_mut().unwrap().body = "The total counted refunds.\n\nAddresses johan's comment on 3f2a9c1.".to_string();
        assert_eq!(fix_message(&read).matches("Addresses johan's").count(), 1, "a body that already carries it does not get it twice");
        assert!(fix_message(&read).ends_with("\n\nAddresses johan's comment on 3f2a9c1."));
        read.author = "jo\nhan; rm".to_string();
        assert_eq!(addresses_line(&read), "Addresses johanrm's comment on 3f2a9c1.", "the login cannot break the message");
    }
}
