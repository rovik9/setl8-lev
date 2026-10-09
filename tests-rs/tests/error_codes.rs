use setl8_lev_trading::errors::Setl8Error;

#[test]
fn error_code_anchors_match_design_section_12() {
    assert_eq!(u32::from(Setl8Error::Unauthorized), 6000);
    assert_eq!(u32::from(Setl8Error::DailyPayoutCapReached), 6024);
    assert_eq!(u32::from(Setl8Error::InvalidLimits), 6034);
    assert_eq!(u32::from(Setl8Error::NotHeld), 6038);
}
