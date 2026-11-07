//! A restart that gives new first values of the inputs, on the model already
//! compiled, starts where a session constructed with them starts.

use super::*;

fn options(solver_mode: SimSolverMode) -> SimOptions {
    SimOptions {
        solver_mode,
        ..SimOptions::default()
    }
}

const INITIAL_FROM_INPUT_MODEL: &str = r#"
model SessionApiInitialFromInput
  input Real u = 1;
  Real x;
initial equation
  x = u;
equation
  der(x) = -x;
end SessionApiInitialFromInput;
"#;

fn started_with(mode: SimSolverMode, u: f64) -> SimulationSession {
    let model = compile(INITIAL_FROM_INPUT_MODEL, "SessionApiInitialFromInput");
    let mut opts = options(mode);
    opts.initial_inputs = vec![("u".to_string(), u)];
    SimulationSession::new(&model, opts).unwrap()
}

/// A restart that gives new first values of the inputs starts where a session
/// constructed with them starts, and goes on the same way.
#[test]
fn reset_with_inputs_starts_from_the_new_inputs_like_a_new_session() {
    for mode in [SimSolverMode::RkLike, SimSolverMode::Bdf] {
        let mut reused = started_with(mode, 3.0);
        assert_eq!(reused.get("x").unwrap(), Some(3.0), "{mode:?}");
        for u in [5.0, 0.5, 3.0] {
            reused.reset_with_inputs(0.0, &[("u", u)]).unwrap();
            let mut fresh = started_with(mode, u);
            assert_eq!(reused.time(), 0.0);
            assert_eq!(reused.get("x").unwrap(), Some(u), "{mode:?}: x(0) after a restart at u = {u}");
            for session in [&mut reused, &mut fresh] {
                session.ensure_end_time(1.0);
                session.advance_to(0.5).unwrap();
            }
            assert_eq!(
                reused.get("x").unwrap().map(f64::to_bits),
                fresh.get("x").unwrap().map(f64::to_bits),
                "{mode:?}: x(0.5) after a restart at u = {u}"
            );
        }
    }
}

#[test]
fn reset_with_inputs_refuses_an_unknown_input_and_moves_nothing() {
    for mode in [SimSolverMode::RkLike, SimSolverMode::Bdf] {
        let mut session = started_with(mode, 3.0);
        session.ensure_end_time(1.0);
        session.advance_to(0.25).unwrap();
        let (time, x) = (session.time(), session.get("x").unwrap());
        assert!(session.reset_with_inputs(0.0, &[("nope", 1.0)]).is_err(), "{mode:?}");
        assert!(session.reset_with_inputs(0.0, &[("u", f64::NAN)]).is_err(), "{mode:?}");
        assert_eq!((session.time(), session.get("x").unwrap()), (time, x), "{mode:?}");
    }
}
