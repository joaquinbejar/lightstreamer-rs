use std::sync::Arc;
use tokio::sync::Notify;
use tracing::info;

/// Clean the message from newlines and carriage returns and convert it to lowercase. Also remove all brackets.
pub fn clean_message(text: &str) -> String {
    let mut result = String::new();
    let mut inside_braces = false;

    for part in text.split_inclusive(&['{', '}']) {
        if part.starts_with('{') && part.ends_with('}') {
            // Part is fully inside braces
            inside_braces = true;
            result.push_str(part);
        } else if inside_braces {
            // We're processing a segment after an opening brace
            inside_braces = false;
            result.push_str(part);
        } else {
            // Process the part outside braces
            let chars_to_replace = ['\n', '\r']; // Using an array of chars to replace
            result.push_str(&part.replace(chars_to_replace, "").to_lowercase());
        }
    }

    result
}

/// Parses a comma-separated string input into a vector of string slices (`Vec<&str>`).
///
/// This function supports skipping commas inside nested curly braces `{}`. It correctly handles
/// nested structures, ensuring that commas within curly braces are not treated as delimiters.
///
/// # Parameters
/// - `input`: A string slice (`&str`) containing comma-separated values, potentially with nested curly braces.
///
/// # Returns
/// A `Vec<&str>` containing trimmed substrings split by commas outside of curly braces.
///
/// # Behavior
/// - Commas outside of curly braces `{}` are treated as delimiters.
/// - Commas inside curly braces are ignored for splitting purposes.
/// - Leading and trailing whitespace around substrings are trimmed.
/// - Empty substrings (those consisting solely of whitespace) are ignored.
///
/// # Caveats
/// - The function requires matched curly braces `{}`. If the input contains unmatched curly braces,
///   the function may produce unexpected results.
///
/// # Panics
/// This function does not explicitly panic, but improper manipulation of indices or unmatched
/// braces could lead to unintended behavior.
///
/// # Errors in Current Code:
/// - There is a bug in the code where `arguments.push(*slice)` is used. The dereference operator
///   (`*`) is invalid for string slices. It should be `arguments.push(slice)`.
/// - Recommend fixing this bug by removing the dereference operator for proper functionality.
pub fn parse_arguments(input: &str) -> Vec<&str> {
    let mut arguments = Vec::new();
    let mut start = 0;
    let mut in_brackets = 0; // Tracks nesting level for curly braces

    for (i, c) in input.char_indices() {
        match c {
            '{' => in_brackets += 1,
            '}' => in_brackets -= 1,
            ',' if in_brackets == 0 => {
                // Outside of brackets, treat comma as a delimiter
                let slice = &input[start..i].trim();
                if !slice.is_empty() {
                    arguments.push(*slice); // Dereference slice here
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    // Push the final argument if it's not empty
    if start < input.len() {
        let slice = &input[start..].trim();
        if !slice.is_empty() {
            arguments.push(*slice); // Dereference slice here
        }
    }

    arguments
}

/// Sets up cross-platform signal handling for graceful shutdown.
/// On Unix systems, handles SIGINT and SIGTERM signals.
/// On Windows, handles Ctrl+C signal.
///
/// # Arguments
///
/// * `shutdown_signal` - `Arc<Notify>` to signal shutdown to other parts of the application.
///
/// # Panics
///
/// The function panics if it fails to create the signal handlers.
///
pub async fn setup_signal_hook(shutdown_signal: Arc<Notify>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to create SIGINT handler");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to create SIGTERM handler");

        tokio::spawn(async move {
            tokio::select! {
                _ = sigint.recv() => {
                    info!("Received SIGINT signal");
                    shutdown_signal.notify_one();
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM signal");
                    shutdown_signal.notify_one();
                }
            }
        });
    }

    #[cfg(windows)]
    {
        use tokio::signal;
        tokio::spawn(async move {
            match signal::ctrl_c().await {
                Ok(()) => {
                    info!("Received Ctrl+C signal");
                    shutdown_signal.notify_one();
                }
                Err(err) => {
                    error!("Failed to listen for Ctrl+C: {}", err);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod clean_message_tests {
        use super::*;

        #[test]
        fn test_clean_message_basic() {
            let text = "Hello\nWorld";
            let result = clean_message(text);
            assert_eq!(result, "helloworld");
        }

        #[test]
        fn test_clean_message_with_partial_braces() {
            // This tests the case where a part starts with '{' but doesn't end with '}'
            let text = "{partial brace content} followed by text";
            let result = clean_message(text);
            assert_eq!(result, "{partial brace content} followed by text");
        }

        #[test]
        fn test_clean_message_with_ending_brace() {
            // This tests the case where a part ends with '}' but doesn't start with '{'
            let text = "text followed by {partial brace content}";
            let result = clean_message(text);
            assert_eq!(result, "text followed by {partial brace content}");
        }

        #[test]
        fn test_clean_message_with_carriage_return() {
            let text = "Hello\r\nWorld";
            let result = clean_message(text);
            assert_eq!(result, "helloworld");
        }

        #[test]
        fn test_clean_message_lowercase_conversion() {
            let text = "Hello WORLD";
            let result = clean_message(text);
            assert_eq!(result, "hello world");
        }

        #[test]
        fn test_clean_message_empty_string() {
            let text = "";
            let result = clean_message(text);
            assert_eq!(result, "");
        }

        #[test]
        fn test_clean_message_preserve_braces_content() {
            let text = "Message with {Preserved\nContent} and not preserved\nContent";
            let result = clean_message(text);
            assert_eq!(
                result,
                "message with {preservedcontent} and not preservedcontent"
            );
        }

        #[test]
        fn test_clean_message_nested_braces() {
            let text = "Message with {Outer{Inner\nContent}Outer} and regular\nContent";
            let result = clean_message(text);
            assert_eq!(
                result,
                "message with {outer{innercontent}outer} and regularcontent"
            );
        }

        #[test]
        fn test_clean_message_unbalanced_braces() {
            let text = "Message with {Unbalanced and regular\nContent";
            let result = clean_message(text);
            assert_eq!(result, "message with {unbalanced and regularcontent");
        }

        #[test]
        fn test_clean_message_protocol_example() {
            // Typical TLCP message examples
            let text = "CONOK,S8f4aec42c3c14ad0,50000,5000,*\r\n";
            let result = clean_message(text);
            assert_eq!(result, "conok,s8f4aec42c3c14ad0,50000,5000,*");

            let text = "PROBE\r\n";
            let result = clean_message(text);
            assert_eq!(result, "probe");
        }
    }

    mod parse_arguments_tests {
        use super::*;

        #[test]
        fn test_parse_arguments_basic() {
            let input = "arg1,arg2,arg3";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["arg1", "arg2", "arg3"]);
        }

        #[test]
        fn test_parse_arguments_empty_string() {
            let input = "";
            let result = parse_arguments(input);
            assert_eq!(result, Vec::<&str>::new());
        }

        #[test]
        fn test_parse_arguments_single_argument() {
            let input = "arg1";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["arg1"]);
        }

        #[test]
        fn test_parse_arguments_with_whitespace() {
            let input = " arg1 , arg2 , arg3 ";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["arg1", "arg2", "arg3"]);
        }

        #[test]
        fn test_parse_arguments_empty_arguments() {
            let input = "arg1,,arg3";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["arg1", "arg3"]);
        }

        #[test]
        fn test_parse_arguments_with_braces() {
            let input = "arg1,{inner1,inner2},arg3";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["arg1", "{inner1,inner2}", "arg3"]);
        }

        #[test]
        fn test_parse_arguments_nested_braces() {
            let input = "arg1,{outer{inner1,inner2}outer},arg3";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["arg1", "{outer{inner1,inner2}outer}", "arg3"]);
        }

        #[test]
        fn test_parse_arguments_unbalanced_braces() {
            let input = "arg1,{unbalanced,arg3";
            let result = parse_arguments(input);
            // Even with unbalanced braces, we expect it to treat everything inside as one argument
            assert_eq!(result, vec!["arg1", "{unbalanced,arg3"]);
        }

        #[test]
        fn test_parse_arguments_protocol_examples() {
            // TLCP protocol message example arguments
            let input = "CONOK,S8f4aec42c3c14ad0,50000,5000,*";
            let result = parse_arguments(input);
            assert_eq!(
                result,
                vec!["CONOK", "S8f4aec42c3c14ad0", "50000", "5000", "*"]
            );

            let input = "u,1,1,a|b|c";
            let result = parse_arguments(input);
            assert_eq!(result, vec!["u", "1", "1", "a|b|c"]);
        }
    }
}
