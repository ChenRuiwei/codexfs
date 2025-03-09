use num_traits::PrimInt;

pub fn round_up<T: PrimInt>(value: T, align: T) -> T {
    (value + align - T::one()) & !(align - T::one())
}

pub fn round_down<T: PrimInt>(value: T, align: T) -> T {
    value & !(align - T::one())
}

pub fn is_dot_or_dotdot(s: &str) -> bool {
    s == "." || s == ".."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_round_up() {
        assert_eq!(round_up(8, 8), 8);
        assert_eq!(round_up(5, 4), 8);
        assert_eq!(round_up(7, 8), 8);
    }

    #[test]
    fn check_round_down() {
        assert_eq!(round_down(8, 8), 8);
        assert_eq!(round_down(5, 4), 4);
        assert_eq!(round_down(7, 8), 0);
    }

    #[test]
    fn check_is_dot_or_dotdot() {
        assert!(is_dot_or_dotdot("."));
        assert!(is_dot_or_dotdot(".."));
        assert!(!is_dot_or_dotdot("..."));
        assert!(!is_dot_or_dotdot("not dot"));
    }
}
