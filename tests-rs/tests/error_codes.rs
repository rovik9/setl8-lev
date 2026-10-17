use setl8_lev_trading::errors::Setl8Error;

#[test]
fn error_code_anchors_match_design_section_12() {
    assert_eq!(u32::from(Setl8Error::Unauthorized), 6000);
    assert_eq!(u32::from(Setl8Error::DailyPayoutCapReached), 6024);
    assert_eq!(u32::from(Setl8Error::InvalidLimits), 6034);
    assert_eq!(u32::from(Setl8Error::NotHeld), 6038);
    assert_eq!(u32::from(Setl8Error::InvalidVaultAccount), 6103);
    assert_eq!(u32::from(Setl8Error::ResetTierInvalid), 6104);
    assert_eq!(u32::from(Setl8Error::ResetPhaseOutOfRange), 6105);
    assert_eq!(u32::from(Setl8Error::ResetPriceOverflow), 6106);
    assert_eq!(u32::from(Setl8Error::AccountAlreadyReset), 6107);
    assert_eq!(u32::from(Setl8Error::ViolationNoReset), 6108);
    assert_eq!(u32::from(Setl8Error::MarketUnpriced), 6109);
}
