// reused-from: binst-pilot-webapp@dda2d1a:src/wallet_picker.rs (2026-11-16) — adapted: icon helpers only, made public
//! Inline SVG icons for known wallets.

// ── Icon helpers ──────────────────────────────────────────────────

/// Inline SVG icons keyed by wallet name — same approach as Web3Modal/RainbowKit.
/// Falls back to a generic icon. Used for both BTC and EVM pickers.
pub fn wallet_svg_icon(name: &str, path_hint: &str) -> String {
    let n = name.to_lowercase();
    let k = path_hint.to_lowercase();

    // MetaMask — fox head (official brand colours)
    if n.contains("metamask") {
        return r##"<svg width="28" height="28" viewBox="0 0 35 33" xmlns="http://www.w3.org/2000/svg"><path d="M32.958 1L19.824 10.718l2.443-5.727L32.958 1Z" fill="#E17726"/><path d="M2.049 1l13.022 9.809-2.33-5.818L2.05 1Z" fill="#E27625"/><path d="M28.229 23.533l-3.495 5.34 7.512 2.06 2.15-7.282-6.167-.118Z" fill="#E27625"/><path d="M.617 23.65l2.137 7.283 7.498-2.06-3.482-5.34L.617 23.65Z" fill="#E27625"/><path d="M9.854 14.711l-2.09 3.117 7.427.337-.254-7.957-5.083 4.503Z" fill="#E27625"/><path d="M25.146 14.711l-5.164-4.594-.182 8.048 7.413-.337-2.067-3.117Z" fill="#E27625"/><path d="M10.252 28.872l4.475-2.172-3.856-2.998-.619 5.17Z" fill="#E27625"/><path d="M20.273 26.7l4.462 2.172-.606-5.17-3.856 2.998Z" fill="#E27625"/></svg>"##.into();
    }
    // SafePal
    if n.contains("safepal") || k.contains("safepal") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#1A1A2E"/><path d="M14 5L21 9.5V18.5L14 23L7 18.5V9.5L14 5Z" fill="none" stroke="#00C4FF" stroke-width="1.5"/><circle cx="14" cy="14" r="3.5" fill="#00C4FF"/></svg>"##.into();
    }
    // Coinbase Wallet
    if n.contains("coinbase") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#0052FF"/><path d="M14 6C9.582 6 6 9.582 6 14s3.582 8 8 8 8-3.582 8-8-3.582-8-8-8Zm-2.5 10.5v-5h5v5h-5Z" fill="white"/></svg>"##.into();
    }
    // Rabby
    if n.contains("rabby") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#8697FF"/><ellipse cx="14" cy="13" rx="6" ry="5" fill="white"/><circle cx="11.5" cy="12" r="1.5" fill="#8697FF"/><circle cx="16.5" cy="12" r="1.5" fill="#8697FF"/><path d="M10 17.5Q14 21 18 17.5" stroke="white" stroke-width="1.5" fill="none" stroke-linecap="round"/></svg>"##.into();
    }
    // OKX
    if n.contains("okx") || k.contains("okxwallet") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#000"/><rect x="7" y="7" width="6" height="6" fill="white"/><rect x="15" y="7" width="6" height="6" fill="white"/><rect x="7" y="15" width="6" height="6" fill="white"/><rect x="15" y="15" width="6" height="6" fill="white"/></svg>"##.into();
    }
    // Trust Wallet
    if n.contains("trust") || k.contains("trustwallet") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#3375BB"/><path d="M14 6l7 3v6c0 4-3.5 7.5-7 8-3.5-.5-7-4-7-8V9l7-3Z" fill="white"/></svg>"##.into();
    }
    // Brave
    if n.contains("brave") || k.contains("brave") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#FF5500"/><path d="M14 5l6 3.5 1 7.5-7 8-7-8 1-7.5L14 5Z" fill="white"/><path d="M14 9l3 2.5-1 4.5-2 2-2-2-1-4.5L14 9Z" fill="#FF5500"/></svg>"##.into();
    }
    // Xverse
    if n.contains("xverse") || k.contains("xverse") || k.contains("bitcoinprovider") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#232336"/><path d="M8 19l6-10 6 10" fill="none" stroke="#EE7A30" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M10.5 15h7" stroke="#EE7A30" stroke-width="2" stroke-linecap="round"/></svg>"##.into();
    }
    // Magic Eden
    if n.contains("magic eden") || k.contains("magiceden") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#E42575"/><text x="14" y="20" text-anchor="middle" fill="white" font-size="14" font-weight="bold">ME</text></svg>"##.into();
    }
    // Phantom
    if n.contains("phantom") || k.contains("phantom") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#AB9FF2"/><path d="M7 14c0-3.866 3.134-7 7-7h2c3.314 0 6 2.686 6 6s-2.686 6-6 6h-1s-1 3-4 3c0-2 1-3 1-3-2.761 0-5-2.239-5-5Z" fill="white"/><circle cx="12.5" cy="14" r="1.5" fill="#AB9FF2"/><circle cx="17.5" cy="14" r="1.5" fill="#AB9FF2"/></svg>"##.into();
    }
    // UniSat
    if n.contains("unisat") || k.contains("unisat") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#FF8C00"/><circle cx="14" cy="14" r="7" fill="none" stroke="white" stroke-width="2"/><text x="14" y="18" text-anchor="middle" fill="white" font-size="9" font-weight="bold">&#x20BF;</text></svg>"##.into();
    }
    // Xverse
    if n.contains("xverse") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#000"/><text x="14" y="20" text-anchor="middle" fill="white" font-size="14" font-weight="bold">X</text></svg>"##.into();
    }
    // Leather / Hiro
    if n.contains("leather") || n.contains("hiro") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#F5A623"/><text x="14" y="20" text-anchor="middle" fill="white" font-size="14" font-weight="bold">L</text></svg>"##.into();
    }
    // Bitget / BitKeep
    if n.contains("bitget") || k.contains("bitkeep") {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#00D4AA"/><text x="14" y="20" text-anchor="middle" fill="white" font-size="11" font-weight="bold">BG</text></svg>"##.into();
    }

    // Generic EVM (Ethereum diamond)
    if k.contains("ethereum") || k.is_empty() {
        return r##"<svg width="28" height="28" viewBox="0 0 28 28" xmlns="http://www.w3.org/2000/svg"><rect width="28" height="28" rx="6" fill="#627EEA"/><path d="M14 5v6.5l5.5 2.5L14 5Z" fill="white" fill-opacity="0.6"/><path d="M14 5L8.5 14l5.5-2.5V5Z" fill="white"/><path d="M14 18.5V23l5.5-8L14 18.5Z" fill="white" fill-opacity="0.6"/><path d="M14 23v-4.5L8.5 15 14 23Z" fill="white"/><path d="M14 17.5l5.5-3.5-5.5-2.5v6Z" fill="white" fill-opacity="0.2"/><path d="M8.5 14l5.5 3.5v-6L8.5 14Z" fill="white" fill-opacity="0.6"/></svg>"##.into();
    }
    // Generic BTC — return empty so caller falls back to obj.icon if available
    String::new()
}

pub fn btc_wallet_icon(path: &str, name: &str) -> String {
    wallet_svg_icon(name, path)
}

pub fn evm_wallet_icon(name: &str) -> String {
    wallet_svg_icon(name, "ethereum")
}
