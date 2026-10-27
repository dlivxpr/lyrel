use crate::address::Context;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::mock;

#[test]
fn the_still_missing_verbs_say_so_without_touching_the_wire() {
    for (args, needle) in [
        (vec!["lyrel", "ws", "stop", "api"], "not implemented"),
        (
            vec!["lyrel", "machine", "connect", "devbox"],
            "not implemented",
        ),
        (
            vec!["lyrel", "machine", "disconnect", "devbox"],
            "not implemented",
        ),
    ] {
        let mut backend = mock();
        let err = execute(cli(&args), &Context::default(), &mut backend)
            .expect_err("stubbed verbs must fail loudly, not pretend");
        assert!(
            err.to_string().contains(needle),
            "{args:?} should mention '{needle}': {err}"
        );
        assert!(
            backend.control_calls.is_empty(),
            "{args:?} must not invent protocol traffic"
        );
    }
}
