//! Phase 5 acceptance (PLAN.md 11.8, ECONOMY.md): the scam test suite. Every test drives the
//! economy through its public API against a real Postgres and ends with the ledger audit.
//! Needs `GM_TEST_DATABASE_URL` (a Postgres the suite may wipe); without it the tests skip.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use gm_core::loot;
use gm_hub::Db;
use gm_hub::economy::{
    EconError, Economy, HIRE_BURN_PER_CENT, INVENTORY_SLOTS, NOT_FOR_HIRE, NOT_HERE, Outcome,
    PRICE_CHANGED, STALL_SLOTS, STORAGE_SLOTS, TradeState, TradeStatus, salvage_indices,
};
use sqlx::Row;

static WIPED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A fresh economy on the test database, or `None` when no database is configured.
async fn setup() -> Option<Economy> {
    let Ok(url) = std::env::var("GM_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: set GM_TEST_DATABASE_URL to a Postgres this suite may wipe");
        return None;
    };
    WIPED
        .get_or_init(|| async {
            let db = Db::connect(&url).await.expect("database");
            db.migrate().await.expect("migrations");
            db.wipe().await.expect("wipe");
            db.pool().close().await;
        })
        .await;
    let db = Db::connect(&url).await.expect("database");
    let mut econ = Economy::new(db.pool().clone());
    econ.trade_cooldown = Duration::from_millis(250);
    // The characters of these tests are rows: they play in no zone (the rule that a
    // trade is between two in one zone is tested over the protocol, tests/party.rs).
    econ.trades_need_a_zone = false;
    Some(econ)
}

/// A new account with one character; returns the character id.
async fn player(econ: &Economy, coin: i64) -> i64 {
    let (_, c) = account_with_character(econ).await;
    if coin > 0 {
        econ.grant_coin(c, coin, 0).await.unwrap();
    }
    c
}

async fn account_with_character(econ: &Economy) -> (i64, i64) {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let account: i64 =
        sqlx::query("insert into accounts (email, password_hash) values ($1, 'x') returning id")
            .bind(format!("p{n}@example.test"))
            .fetch_one(econ_pool(econ))
            .await
            .unwrap()
            .try_get("id")
            .unwrap();
    (account, character_of(econ, account).await)
}

async fn character_of(econ: &Economy, account: i64) -> i64 {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    sqlx::query("insert into characters (account_id, name, name_key, build) values ($1, $2, lower($2), '{}'::jsonb) returning id")
        .bind(account)
        .bind(format!("Char{n}"))
        .fetch_one(econ_pool(econ))
        .await
        .unwrap()
        .try_get("id")
        .unwrap()
}

fn econ_pool(econ: &Economy) -> &sqlx::PgPool {
    econ.pool()
}

async fn component(econ: &Economy, character: i64, material: &str) -> i64 {
    econ.grant_components("test_zone", &[(character, material.to_string())], 0)
        .await
        .unwrap()[0]
}

async fn coin(econ: &Economy, character: i64) -> i64 {
    econ.inventory(character).await.unwrap().0
}

async fn has(econ: &Economy, character: i64, item: i64) -> bool {
    econ.inventory(character)
        .await
        .unwrap()
        .1
        .iter()
        .any(|i| i.id == item)
}

async fn sound(econ: &Economy) {
    assert_eq!(
        econ.audit().await.unwrap(),
        0,
        "the ledger disagrees with the balances"
    );
}

async fn cooldown(econ: &Economy) {
    tokio::time::sleep(econ.trade_cooldown + Duration::from_millis(60)).await;
}

// ---------- the trade window ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trade_swap_scam_is_refused_by_the_mutation_lock() {
    let Some(econ) = setup().await else { return };
    let scammer = player(&econ, 0).await;
    let victim = player(&econ, 5_000).await;
    let relic = component(&econ, scammer, "core/dragonbone").await;
    let junk = component(&econ, scammer, "core/tin").await;

    let t = econ.trade_open(scammer, victim).await.unwrap();
    econ.trade_offer_item(t, scammer, relic).await.unwrap();
    econ.trade_set_coin(t, victim, 5_000).await.unwrap();
    let seen = econ.trade_version(t).await.unwrap();

    // Accepting inside the cooldown is refused.
    assert_eq!(
        econ.trade_accept(t, victim, seen).await,
        Err(EconError::Cooldown)
    );
    cooldown(&econ).await;
    assert_eq!(
        econ.trade_accept(t, victim, seen).await,
        Ok(TradeStatus::Waiting)
    );

    // The swap: the relic out, the junk in. Both accepts are gone and the cooldown restarts.
    econ.trade_retract_item(t, scammer, relic).await.unwrap();
    econ.trade_offer_item(t, scammer, junk).await.unwrap();
    assert_eq!(
        econ.trade_accept(t, scammer, econ.trade_version(t).await.unwrap())
            .await,
        Err(EconError::Cooldown)
    );
    cooldown(&econ).await;
    // The scammer accepts; the victim's earlier accept does not count, so nothing commits.
    let now = econ.trade_version(t).await.unwrap();
    assert_eq!(
        econ.trade_accept(t, scammer, now).await,
        Ok(TradeStatus::Waiting)
    );
    // A lagging victim still showing the old offer cannot accept blind.
    assert!(matches!(
        econ.trade_accept(t, victim, seen).await,
        Err(EconError::State(_))
    ));
    assert_eq!(coin(&econ, victim).await, 5_000);
    assert!(has(&econ, scammer, relic).await && has(&econ, scammer, junk).await);

    // The honest version of the same trade goes through.
    econ.trade_retract_item(t, scammer, junk).await.unwrap();
    econ.trade_offer_item(t, scammer, relic).await.unwrap();
    cooldown(&econ).await;
    let v = econ.trade_version(t).await.unwrap();
    assert_eq!(
        econ.trade_accept(t, victim, v).await,
        Ok(TradeStatus::Waiting)
    );
    assert_eq!(
        econ.trade_accept(t, scammer, v).await,
        Ok(TradeStatus::Committed)
    );
    assert!(has(&econ, victim, relic).await);
    assert_eq!(coin(&econ, scammer).await, 5_000);
    assert_eq!(coin(&econ, victim).await, 0);
    // A committed trade is closed for good.
    assert!(matches!(
        econ.trade_accept(t, victim, v).await,
        Err(EconError::State(_))
    ));
    assert!(matches!(
        econ.trade_set_coin(t, scammer, 1).await,
        Err(EconError::State(_))
    ));
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trade_commit_verifies_everything_again() {
    let Some(econ) = setup().await else { return };
    let a = player(&econ, 100).await;
    let b = player(&econ, 100).await;
    let c = player(&econ, 0).await;
    let item = component(&econ, a, "frame/ash").await;

    // The same item offered in two trades: the first commit wins, the second finds it gone.
    let t1 = econ.trade_open(a, b).await.unwrap();
    let t2 = econ.trade_open(a, c).await.unwrap();
    econ.trade_offer_item(t1, a, item).await.unwrap();
    econ.trade_offer_item(t2, a, item).await.unwrap();
    // Offering what you do not hold is refused outright.
    assert_eq!(
        econ.trade_offer_item(t1, b, item).await,
        Err(EconError::Forbidden)
    );
    cooldown(&econ).await;
    let v1 = econ.trade_version(t1).await.unwrap();
    let v2 = econ.trade_version(t2).await.unwrap();
    econ.trade_accept(t1, a, v1).await.unwrap();
    econ.trade_accept(t2, a, v2).await.unwrap();
    assert_eq!(
        econ.trade_accept(t1, b, v1).await,
        Ok(TradeStatus::Committed)
    );
    assert!(matches!(
        econ.trade_accept(t2, c, v2).await,
        Err(EconError::State(_))
    ));
    assert!(has(&econ, b, item).await && !has(&econ, c, item).await);

    // Coin promised but spent before the commit: nothing moves.
    let t3 = econ.trade_open(a, b).await.unwrap();
    econ.trade_set_coin(t3, a, 100).await.unwrap();
    econ.trade_offer_item(t3, b, item).await.unwrap();
    cooldown(&econ).await;
    let v3 = econ.trade_version(t3).await.unwrap();
    econ.trade_accept(t3, b, v3).await.unwrap();
    // `a` moves the coin away through another committed trade.
    let t4 = econ.trade_open(a, c).await.unwrap();
    econ.trade_set_coin(t4, a, 60).await.unwrap();
    cooldown(&econ).await;
    let v4 = econ.trade_version(t4).await.unwrap();
    econ.trade_accept(t4, a, v4).await.unwrap();
    assert_eq!(
        econ.trade_accept(t4, c, v4).await,
        Ok(TradeStatus::Committed)
    );
    assert_eq!(
        econ.trade_accept(t3, a, v3).await,
        Err(EconError::Insufficient)
    );
    assert!(has(&econ, b, item).await);
    assert_eq!(coin(&econ, a).await, 40);

    // An item destroyed while on offer leaves the trade and clears the accepts.
    let core = component(&econ, a, "core/iron").await;
    let frame = component(&econ, a, "frame/oak").await;
    let t5 = econ.trade_open(a, b).await.unwrap();
    econ.trade_offer_item(t5, a, core).await.unwrap();
    cooldown(&econ).await;
    let v5 = econ.trade_version(t5).await.unwrap();
    econ.trade_accept(t5, b, v5).await.unwrap();
    econ.craft(a, "sword", &[core, frame], None).await.unwrap();
    assert!(matches!(
        econ.trade_accept(t5, a, v5).await,
        Err(EconError::State(_))
    ));
    assert!(econ.trade_version(t5).await.unwrap() > v5);
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trade_respects_capacity_as_a_net_change() {
    let Some(econ) = setup().await else { return };
    let a = player(&econ, 0).await;
    let b = player(&econ, 0).await;
    let mut a_items = Vec::new();
    for _ in 0..INVENTORY_SLOTS {
        a_items.push(component(&econ, a, "gem/quartz").await);
    }
    let b_item = component(&econ, b, "gem/opal").await;
    // `a` is full but gives two and takes one: a net of minus one, so it fits.
    let t = econ.trade_open(a, b).await.unwrap();
    econ.trade_offer_item(t, a, a_items[0]).await.unwrap();
    econ.trade_offer_item(t, a, a_items[1]).await.unwrap();
    econ.trade_offer_item(t, b, b_item).await.unwrap();
    cooldown(&econ).await;
    let v = econ.trade_version(t).await.unwrap();
    econ.trade_accept(t, a, v).await.unwrap();
    assert_eq!(econ.trade_accept(t, b, v).await, Ok(TradeStatus::Committed));
    // A gift into a full inventory does not.
    let gift = component(&econ, b, "gem/opal").await;
    let extra = component(&econ, b, "gem/opal").await;
    let t = econ.trade_open(a, b).await.unwrap();
    econ.trade_offer_item(t, b, gift).await.unwrap();
    econ.trade_offer_item(t, b, extra).await.unwrap();
    cooldown(&econ).await;
    let v = econ.trade_version(t).await.unwrap();
    econ.trade_accept(t, a, v).await.unwrap();
    assert_eq!(econ.trade_accept(t, b, v).await, Err(EconError::Full));
    assert!(has(&econ, b, gift).await);
    sound(&econ).await;
}

// ---------- stalls ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalls_snap_to_the_grid_and_sell_exactly_once() {
    let Some(econ) = setup().await else { return };
    let seller = player(&econ, 0).await;
    let rival = player(&econ, 0).await;
    let stall = econ.stall_open(seller, "stall_town", 4, 7).await.unwrap();
    // The stall-blocking scam: the tile is taken, by constraint.
    assert!(matches!(
        econ.stall_open(rival, "stall_town", 4, 7).await,
        Err(EconError::State(_))
    ));
    // One stall per character, said before the insert (a keeper asking again after a
    // restart must not leave a constraint violation in the database log each second).
    assert!(matches!(
        econ.stall_open(seller, "stall_town", 5, 7).await,
        Err(EconError::State(why)) if why == "you already have a stall"
    ));
    let rivals = econ.stall_open(rival, "stall_town", 5, 7).await.unwrap();

    let item = component(&econ, seller, "catalyst/ember").await;
    // A stall is kept where it stands: listing names the zone its keeper plays in, and a
    // stall that stands elsewhere is not found from there.
    assert_eq!(
        econ.stall_list(seller, "another_town", item, 300).await,
        Err(EconError::NotFound)
    );
    let listing = econ
        .stall_list(seller, "stall_town", item, 300)
        .await
        .unwrap();
    // A listing comes back out (a price put wrong is put right): only its keeper's, only
    // from the stall's zone, and only once.
    assert_eq!(
        econ.stall_unlist(rival, "stall_town", listing).await,
        Err(EconError::NotFound),
        "not a listing of the rival's stall"
    );
    assert_eq!(
        econ.stall_unlist(seller, "another_town", listing).await,
        Err(EconError::NotFound)
    );
    econ.stall_unlist(seller, "stall_town", listing)
        .await
        .unwrap();
    assert!(has(&econ, seller, item).await, "back in the inventory");
    assert_eq!(
        econ.stall_unlist(seller, "stall_town", listing).await,
        Err(EconError::NotFound)
    );
    assert!(econ.stall_view(stall).await.unwrap().2.is_empty());
    let listing = econ
        .stall_list(seller, "stall_town", item, 300)
        .await
        .unwrap();
    assert!(
        !has(&econ, seller, item).await,
        "a listed item sits in the stall"
    );
    assert_eq!(
        econ.stall_buy(seller, "stall_town", stall, listing, 300)
            .await,
        Err(EconError::Invalid("that is your own stall".into()))
    );

    // Eight buyers race for one listing: one wins, nobody else pays.
    let mut buyers = Vec::new();
    for _ in 0..8 {
        buyers.push(player(&econ, 300).await);
    }
    // A buyer shown another price is refused (the price-swap scam).
    assert!(matches!(
        econ.stall_buy(buyers[0], "stall_town", stall, listing, 30)
            .await,
        Err(EconError::State(_))
    ));
    // A stall is a place (ITEMS.md 1): the listing must be of the stall the buyer's zone
    // saw it standing at, and that stall must be one of that zone's.
    assert_eq!(
        econ.stall_buy(buyers[0], "stall_town", rivals, listing, 300)
            .await,
        Err(EconError::NotFound),
        "standing at the stall next door"
    );
    assert_eq!(
        econ.stall_buy(buyers[0], "another_town", stall, listing, 300)
            .await,
        Err(EconError::NotFound),
        "a zone the stall is not in"
    );
    // What a stall shows: its keeper and its listings, from anywhere.
    let (owner, _name, listings) = econ.stall_view(stall).await.unwrap();
    assert_eq!(owner, seller);
    assert_eq!(
        listings
            .iter()
            .map(|l| (l.id, l.item.id, l.price))
            .collect::<Vec<_>>(),
        vec![(listing, item, 300)]
    );
    assert_eq!(listings[0].item.components[0].material, "catalyst/ember");
    assert!(econ.stall_view(rivals).await.unwrap().2.is_empty());
    assert_eq!(econ.stall_view(stall + 99).await, Err(EconError::NotFound));
    let mut tasks = Vec::new();
    for &b in &buyers {
        let econ = econ.clone();
        tasks.push(tokio::spawn(async move {
            econ.stall_buy(b, "stall_town", stall, listing, 300).await
        }));
    }
    let mut won = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            won += 1;
        }
    }
    assert_eq!(won, 1);
    let mut owners = 0;
    let mut paid = 0;
    for &b in &buyers {
        if has(&econ, b, item).await {
            owners += 1;
        }
        paid += 300 - coin(&econ, b).await;
    }
    assert_eq!((owners, paid), (1, 300));
    assert_eq!(
        coin(&econ, seller).await,
        300,
        "no tax and no fee: the seller gets the price"
    );
    // A buyer who cannot pay gets nothing and the listing stays.
    let item2 = component(&econ, seller, "catalyst/ember").await;
    let listing2 = econ
        .stall_list(seller, "stall_town", item2, 1_000)
        .await
        .unwrap();
    assert_eq!(
        econ.stall_buy(buyers[0], "stall_town", stall, listing2, 1_000)
            .await,
        Err(EconError::Insufficient)
    );
    econ.stall_close(seller).await.unwrap();
    assert!(has(&econ, seller, item2).await);
    econ.stall_close(rival).await.unwrap();
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buy_orders_escrow_the_coin_and_pay_on_delivery() {
    let Some(econ) = setup().await else { return };
    let owner = player(&econ, 1_000).await;
    let seller = player(&econ, 0).await;
    econ.stall_open(owner, "orders_town", 0, 0).await.unwrap();
    // An order the owner cannot fund is refused; so is an overflowing one.
    assert_eq!(
        econ.buy_order_post(owner, "core/iron", 600, 2).await,
        Err(EconError::Insufficient)
    );
    assert!(matches!(
        econ.buy_order_post(owner, "core/iron", i64::MAX / 2, 1000)
            .await,
        Err(EconError::Invalid(_))
    ));
    let order = econ
        .buy_order_post(owner, "core/iron", 200, 3)
        .await
        .unwrap();
    assert_eq!(
        coin(&econ, owner).await,
        400,
        "the coin is escrowed when the order is posted"
    );

    let wrong = component(&econ, seller, "core/tin").await;
    let right = component(&econ, seller, "core/iron").await;
    assert!(matches!(
        econ.buy_order_fill(seller, order, wrong).await,
        Err(EconError::Invalid(_))
    ));
    econ.buy_order_fill(seller, order, right).await.unwrap();
    assert_eq!(coin(&econ, seller).await, 200);
    // The same item cannot be delivered twice.
    assert_eq!(
        econ.buy_order_fill(seller, order, right).await,
        Err(EconError::Forbidden)
    );

    // Cancelling returns exactly what is left; closing returns the delivered item.
    econ.buy_order_cancel(owner, order).await.unwrap();
    assert_eq!(coin(&econ, owner).await, 800);
    econ.stall_close(owner).await.unwrap();
    assert!(has(&econ, owner, right).await);
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_owner_cannot_keep_a_stall_tile() {
    let Some(econ) = setup().await else { return };
    let owner = player(&econ, 0).await;
    econ.stall_open(owner, "full_town", 1, 1).await.unwrap();
    let mut listed = Vec::new();
    for _ in 0..STALL_SLOTS {
        let item = component(&econ, owner, "shard/glass").await;
        econ.stall_list(owner, "full_town", item, 999_999)
            .await
            .unwrap();
        listed.push(item);
    }
    let extra = component(&econ, owner, "shard/glass").await;
    assert_eq!(
        econ.stall_list(owner, "full_town", extra, 1).await,
        Err(EconError::Full)
    );
    // Fill the storage and the inventory with junk.
    for _ in 0..STORAGE_SLOTS {
        let j = component(&econ, owner, "shard/glass").await;
        econ.storage_deposit(owner, j).await.unwrap();
    }
    let j = component(&econ, owner, "shard/glass").await;
    assert_eq!(econ.storage_deposit(owner, j).await, Err(EconError::Full));
    while econ.inventory(owner).await.unwrap().1.len() < INVENTORY_SLOTS as usize {
        component(&econ, owner, "shard/glass").await;
    }
    // Expiry still closes the stall: the items overflow into storage and the tile is free.
    econ.stall_close(owner).await.unwrap();
    let other = player(&econ, 0).await;
    econ.stall_open(other, "full_town", 1, 1).await.unwrap();
    let stored = econ.storage(owner).await.unwrap().1.len();
    assert_eq!(stored, (STORAGE_SLOTS + STALL_SLOTS) as usize);
    // No new stall and no deposits until the overflow is cleared: a stall is not storage.
    assert_eq!(
        econ.stall_open(owner, "full_town", 2, 1).await,
        Err(EconError::Full)
    );
    assert_eq!(
        econ.storage_deposit(owner, extra).await,
        Err(EconError::Full)
    );
    econ.stall_close(other).await.unwrap();
    sound(&econ).await;
}

// ---------- escrow contracts ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn carry_escrow_pays_only_on_completion() {
    let Some(econ) = setup().await else { return };
    let buyer = player(&econ, 1_000).await;
    let leader = player(&econ, 500).await;
    let s2 = player(&econ, 0).await;
    let s3 = player(&econ, 0).await;

    // "Pay first and get kicked": nothing is paid until the zone reports the kill.
    let c = econ
        .contract_post(buyer, "snow_fort", 1_000, 200)
        .await
        .unwrap();
    assert!(matches!(
        econ.contract_accept(leader, c, &[leader, buyer]).await,
        Err(EconError::Invalid(_))
    ));
    econ.contract_accept(leader, c, &[leader, s2, s3])
        .await
        .unwrap();
    assert_eq!(
        (coin(&econ, buyer).await, coin(&econ, leader).await),
        (0, 300)
    );
    // Nobody can cancel mid-run, and it cannot be accepted twice.
    assert!(econ.contract_cancel(buyer, c).await.is_err());
    assert!(matches!(
        econ.contract_accept(s2, c, &[s2]).await,
        Err(EconError::State(_))
    ));

    assert!(econ.contract_report(c, Outcome::Completed).await.unwrap());
    // 1,000 over three sellers: 333 each, the remainder to the leader, collateral back.
    assert_eq!(coin(&econ, leader).await, 300 + 334 + 200);
    assert_eq!((coin(&econ, s2).await, coin(&econ, s3).await), (333, 333));
    // A replayed or late contrary report changes nothing.
    assert!(!econ.contract_report(c, Outcome::Completed).await.unwrap());
    assert!(!econ.contract_report(c, Outcome::Abandon).await.unwrap());
    assert_eq!(coin(&econ, buyer).await, 0);
    assert_eq!(econ.contract_state(c).await.unwrap(), "paid");
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn carry_escrow_refunds_on_wipe_abandon_and_stalling() {
    let Some(econ) = setup().await else { return };
    let buyer = player(&econ, 900).await;
    let leader = player(&econ, 300).await;

    // A buyer who cannot pay never activates a contract.
    let c = econ
        .contract_post(buyer, "snow_fort", 5_000, 0)
        .await
        .unwrap();
    assert_eq!(
        econ.contract_accept(leader, c, &[leader]).await,
        Err(EconError::Insufficient)
    );
    assert_eq!(econ.contract_state(c).await.unwrap(), "open");
    econ.contract_cancel(buyer, c).await.unwrap();

    // Wipe: the price back to the buyer, the collateral back to the leader.
    let c = econ
        .contract_post(buyer, "snow_fort", 300, 100)
        .await
        .unwrap();
    econ.contract_accept(leader, c, &[leader]).await.unwrap();
    assert!(econ.contract_report(c, Outcome::Wipe).await.unwrap());
    assert_eq!(
        (coin(&econ, buyer).await, coin(&econ, leader).await),
        (900, 300)
    );

    // Abandon: the buyer takes the collateral too.
    let c = econ
        .contract_post(buyer, "snow_fort", 300, 100)
        .await
        .unwrap();
    econ.contract_accept(leader, c, &[leader]).await.unwrap();
    assert!(econ.contract_report(c, Outcome::Abandon).await.unwrap());
    assert_eq!(
        (coin(&econ, buyer).await, coin(&econ, leader).await),
        (1_000, 200)
    );

    // Stalling forever: after two hours without a report the hub refunds as an abandon.
    let c = econ
        .contract_post(buyer, "snow_fort", 300, 100)
        .await
        .unwrap();
    econ.contract_accept(leader, c, &[leader]).await.unwrap();
    assert_eq!(econ.contracts_expire().await.unwrap(), 0);
    sqlx::query("update contracts set started = now() - interval '121 minutes' where id = $1")
        .bind(c)
        .execute(econ.pool())
        .await
        .unwrap();
    assert!(econ.contracts_expire().await.unwrap() >= 1);
    assert_eq!(econ.contract_state(c).await.unwrap(), "refunded");
    assert_eq!(
        (coin(&econ, buyer).await, coin(&econ, leader).await),
        (1_100, 100)
    );
    sound(&econ).await;
}

// ---------- drops, crafting, storage ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boss_drops_follow_the_corrected_split() {
    let Some(econ) = setup().await else { return };
    let mut ids = Vec::new();
    for _ in 0..6 {
        ids.push(player(&econ, 0).await);
    }
    let m = |i: usize, contribution: u64| loot::Member {
        id: ids[i] as u64,
        contribution,
        damage: contribution,
    };
    let parties = [
        loot::Party {
            id: 1,
            living_member_present: true,
            members: vec![m(0, 500), m(1, 400), m(2, 30)],
        },
        // Wiped just before the kill.
        loot::Party {
            id: 2,
            living_member_present: false,
            members: vec![m(3, 2_000)],
        },
        loot::Party {
            id: 3,
            living_member_present: true,
            members: vec![m(4, 310)],
        },
        // A tagger.
        loot::Party {
            id: 4,
            living_member_present: true,
            members: vec![m(5, 20)],
        },
    ];
    let shares = loot::split(8, &parties);
    let grants: Vec<(i64, String)> = shares
        .iter()
        .flat_map(|&(id, n)| (0..n).map(move |_| (id as i64, "shard/boss_scale".to_string())))
        .collect();
    assert_eq!(
        grants.len(),
        8,
        "drops are deterministic: all eight are handed out"
    );
    econ.grant_components("boss_zone", &grants, 77)
        .await
        .unwrap();
    let mut counts = Vec::new();
    for &id in &ids {
        counts.push(econ.inventory(id).await.unwrap().1.len());
    }
    // The wiped party, the leech (30 of a 310 average) and the tagger get nothing.
    assert_eq!(counts[3] + counts[2] + counts[5], 0);
    assert_eq!(counts.iter().sum::<usize>(), 8);
    assert!(counts[0] >= counts[1] && counts[1] >= 1 && counts[4] >= 1);
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crafting_and_decomposition_never_create_anything() {
    let Some(econ) = setup().await else { return };
    let p = player(&econ, 0).await;
    let core = component(&econ, p, "core/iron").await;
    let frame = component(&econ, p, "frame/oak").await;
    let shard = component(&econ, p, "shard/boss_scale").await;
    let catalyst = component(&econ, p, "catalyst/ember").await;
    let gem1 = component(&econ, p, "gem/opal").await;
    let gem2 = component(&econ, p, "gem/opal").await;
    let gem3 = component(&econ, p, "gem/opal").await;

    // Invalid recipes are refused and consume nothing.
    assert!(econ.craft(p, "sword", &[core], None).await.is_err());
    assert!(
        econ.craft(p, "sword", &[core, frame, gem1, gem2, gem3], None)
            .await
            .is_err()
    );
    assert!(
        econ.craft(p, "sword", &[core, core, frame], None)
            .await
            .is_err()
    );
    assert!(
        econ.craft(p, "component", &[core, frame], None)
            .await
            .is_err()
    );
    let other = player(&econ, 0).await;
    assert_eq!(
        econ.craft(other, "sword", &[core, frame], None).await,
        Err(EconError::Forbidden)
    );
    assert_eq!(econ.inventory(p).await.unwrap().1.len(), 7);

    let sword = econ
        .craft(
            p,
            "sword",
            &[core, frame, shard, catalyst, gem1, gem2],
            None,
        )
        .await
        .unwrap();
    let (_, items) = econ.inventory(p).await.unwrap();
    assert_eq!(items.len(), 2, "six components became one item");
    assert_eq!(
        items
            .iter()
            .find(|i| i.id == sword)
            .unwrap()
            .components
            .len(),
        6
    );
    // The consumed components are gone: they cannot be used or traded again.
    assert!(econ.craft(p, "axe", &[core, frame], None).await.is_err());
    // A template has room for some layers and not for others (ITEMS.md 3.2): a part of a
    // layer it has no room for is refused, and nothing is consumed.
    let core2 = component(&econ, p, "core/iron").await;
    let frame2 = component(&econ, p, "frame/oak").await;
    let ember = component(&econ, p, "catalyst/ember").await;
    let room: Vec<String> = ["shard", "core", "frame", "gem"]
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(
        econ.craft(p, "cuirass", &[core2, frame2, ember], Some(&room))
            .await,
        Err(EconError::Invalid("a cuirass takes no catalyst".into()))
    );
    assert!(has(&econ, p, ember).await && has(&econ, p, core2).await);
    let cuirass = econ
        .craft(p, "cuirass", &[core2, frame2], Some(&room))
        .await
        .unwrap();
    assert!(has(&econ, p, cuirass).await);

    // Decomposition returns half, rounded down, and the item is gone.
    let back = econ.decompose(p, sword).await.unwrap();
    assert_eq!(back.len(), 3);
    assert!(econ.decompose(p, sword).await.is_err());
    assert!(
        econ.decompose(p, back[0]).await.is_err(),
        "a lone component does not decompose"
    );
    // The three that came back, one part bought apart earlier, the cuirass and the ember
    // it had no room for.
    assert_eq!(econ.inventory(p).await.unwrap().1.len(), 6);

    // Padding a craft with junk never raises what comes back above half.
    for item in 0..2_000i64 {
        for k in 2..=6usize {
            let kept = salvage_indices(item, k);
            assert_eq!(kept.len(), k / 2);
        }
    }
    // And which layer survives is not the crafter's choice: over many item ids every index
    // of a two-part item survives about half the time.
    let first = (0..2_000i64)
        .filter(|&i| salvage_indices(i, 2) == vec![0])
        .count();
    assert!((800..=1_200).contains(&first), "{first}");
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn storage_is_per_account_and_capped_under_concurrency() {
    let Some(econ) = setup().await else { return };
    let (account, main) = account_with_character(&econ).await;
    let alt = character_of(&econ, account).await;
    let stranger = player(&econ, 0).await;

    // The mule fix: an item stored by one character is taken out by another of the account.
    let item = component(&econ, main, "core/iron").await;
    econ.storage_deposit(main, item).await.unwrap();
    assert_eq!(
        econ.storage_withdraw(stranger, item).await,
        Err(EconError::Forbidden)
    );
    econ.storage_withdraw(alt, item).await.unwrap();
    assert!(has(&econ, alt, item).await);
    econ.storage_deposit(alt, item).await.unwrap();

    // 59 more fill it; then both characters race to deposit: the cap holds.
    for _ in 0..(STORAGE_SLOTS - 3) {
        let j = component(&econ, main, "gem/quartz").await;
        econ.storage_deposit(main, j).await.unwrap();
    }
    let mut tasks = Vec::new();
    for i in 0..6 {
        let who = if i % 2 == 0 { main } else { alt };
        let j = component(&econ, who, "gem/quartz").await;
        let econ = econ.clone();
        tasks.push(tokio::spawn(
            async move { econ.storage_deposit(who, j).await },
        ));
    }
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(ok, 2);
    assert_eq!(
        econ.storage(main).await.unwrap().1.len(),
        STORAGE_SLOTS as usize
    );

    // A drop into a full inventory lands on the ground of the zone, not nowhere.
    let full = player(&econ, 0).await;
    for _ in 0..INVENTORY_SLOTS {
        component(&econ, full, "gem/quartz").await;
    }
    let spilled = component(&econ, full, "gem/opal").await;
    assert!(!has(&econ, full, spilled).await);
    assert_eq!(
        econ.pickup(stranger, spilled, "elsewhere").await,
        Err(EconError::Forbidden)
    );
    econ.pickup(stranger, spilled, "test_zone").await.unwrap();
    assert!(has(&econ, stranger, spilled).await);
    econ.drop_item(stranger, spilled, "test_zone")
        .await
        .unwrap();
    assert!(!has(&econ, stranger, spilled).await);
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guild_chests_gate_withdrawals_by_rank() {
    let Some(econ) = setup().await else { return };
    let founder = player(&econ, 0).await;
    let recruit = player(&econ, 0).await;
    let outsider = player(&econ, 0).await;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let guild = econ
        .guild_create(&format!("Guild {n}"), founder)
        .await
        .unwrap();
    econ.guild_add(guild, recruit, 0).await.unwrap();
    let open = econ.chest_create(guild, 0, 0).await.unwrap();
    let vault = econ.chest_create(guild, 5, 0).await.unwrap();

    let a = component(&econ, recruit, "core/iron").await;
    let b = component(&econ, recruit, "core/iron").await;
    let c = component(&econ, outsider, "core/iron").await;
    econ.chest_deposit(recruit, open, a).await.unwrap();
    econ.chest_deposit(recruit, vault, b).await.unwrap();
    assert_eq!(
        econ.chest_deposit(outsider, open, c).await,
        Err(EconError::Forbidden)
    );
    assert_eq!(
        econ.chest_withdraw(outsider, open, a).await,
        Err(EconError::Forbidden)
    );
    // Walk in, see it, take it: the open chest serves any member; the vault needs rank.
    assert_eq!(
        econ.chest_withdraw(recruit, vault, b).await,
        Err(EconError::Forbidden)
    );
    econ.chest_withdraw(founder, vault, b).await.unwrap();
    econ.chest_withdraw(founder, open, a).await.unwrap();
    assert!(has(&econ, founder, a).await && has(&econ, founder, b).await);
    sound(&econ).await;
}

// ---------- hires and the money supply ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tavern_hires_burn_thirty_per_cent() {
    let Some(econ) = setup().await else { return };
    let hirer = player(&econ, 10_000).await;
    let (avatar_account, avatar) = account_with_character(&econ).await;
    let own_alt = character_of(&econ, avatar_account).await;
    econ.grant_coin(own_alt, 5_000, 0).await.unwrap();
    econ.hire_list(avatar, 1_000).await.unwrap();

    let before = econ.supply().await.unwrap();
    let (hire, burned) = econ.hire(hirer, avatar, 3, None, |_| true).await.unwrap();
    assert_eq!(burned, 1_000 * HIRE_BURN_PER_CENT / 100);
    assert_eq!(coin(&econ, hirer).await, 9_000);
    assert_eq!(coin(&econ, avatar).await, 700, "flat coin only");
    // Other tests run beside this one, so the supply is checked through this hire's rows.
    let row = sqlx::query("select sum(amount)::bigint as n from coin_ledger where reason = 'hire_burn' and from_holder = (select id from holders where character_id = $1 and kind = 'character')")
        .bind(hirer)
        .fetch_one(econ.pool())
        .await
        .unwrap();
    assert_eq!(row.try_get::<i64, _>("n").unwrap(), 300);
    assert!(econ.supply().await.unwrap().burned >= before.burned + 300);

    // Hiring your own character and hiring an avatar its owner is playing are refused.
    assert!(matches!(
        econ.hire(own_alt, avatar, 3, None, |_| true).await,
        Err(EconError::Invalid(_))
    ));
    sqlx::query("update characters set location_kind = 'zone', location_zone = 'z' where id = $1")
        .bind(avatar)
        .execute(econ.pool())
        .await
        .unwrap();
    let second = player(&econ, 10_000).await;
    assert!(matches!(
        econ.hire(second, avatar, 3, None, |_| true).await,
        Err(EconError::State(_))
    ));
    assert!(
        !econ
            .tavern()
            .await
            .unwrap()
            .iter()
            .any(|t| t.character == avatar)
    );
    sqlx::query(
        "update characters set location_kind = 'offline', location_zone = null where id = $1",
    )
    .bind(avatar)
    .execute(econ.pool())
    .await
    .unwrap();

    // Diminishing priority: after three hires in the window the avatar sorts behind a
    // dearer one that was not hired.
    let (_, fresh) = account_with_character(&econ).await;
    econ.hire_list(fresh, 5_000).await.unwrap();
    let third = player(&econ, 10_000).await;
    econ.hire(second, avatar, 3, None, |_| true).await.unwrap();
    econ.hire(third, avatar, 3, None, |_| true).await.unwrap();
    let list = econ.tavern().await.unwrap();
    let pos = |c: i64| list.iter().position(|t| t.character == c).unwrap();
    assert!(pos(fresh) < pos(avatar));
    assert_eq!(list[pos(avatar)].hires, 3);
    assert!(list[pos(avatar)].name.starts_with("Char"));
    // A hirer without the coin hires nobody.
    let poor = player(&econ, 10).await;
    assert_eq!(
        econ.hire(poor, fresh, 3, None, |_| true).await,
        Err(EconError::Insufficient)
    );
    assert_eq!(coin(&econ, poor).await, 10);
    assert!(econ.squad(poor).await.unwrap().is_empty());

    // The squad (COMPANIONS.md 3.3): the hire is active, the avatar may serve several
    // hirers, and one hirer cannot hold two copies of it.
    let squad = econ.squad(hirer).await.unwrap();
    assert_eq!(squad.len(), 1);
    assert_eq!((squad[0].id, squad[0].avatar), (hire, avatar));
    assert_eq!(econ.squad(second).await.unwrap().len(), 1);
    assert!(matches!(
        econ.hire(hirer, avatar, 3, None, |_| true).await,
        Err(EconError::State(_))
    ));
    // A full squad refuses before any coin moves.
    assert!(matches!(
        econ.hire(hirer, fresh, 1, None, |_| true).await,
        Err(EconError::State(_))
    ));
    assert_eq!(coin(&econ, hirer).await, 9_000);
    // Dismissing frees the slot and refunds nothing; only the hirer can.
    assert_eq!(econ.dismiss(second, hire).await, Err(EconError::NotFound));
    econ.dismiss(hirer, hire).await.unwrap();
    assert_eq!(econ.dismiss(hirer, hire).await, Err(EconError::NotFound));
    assert!(econ.squad(hirer).await.unwrap().is_empty());
    assert_eq!(coin(&econ, hirer).await, 9_000);
    econ.hire(hirer, fresh, 1, None, |_| true).await.unwrap();
    assert_eq!(coin(&econ, hirer).await, 4_000);
    // The owner takes the avatar back: every active hire of it ends, nobody is refunded.
    let mut ended = econ.end_hires_of(avatar).await.unwrap();
    ended.sort_by_key(|(_, hirer)| *hirer);
    assert_eq!(
        ended.iter().map(|(_, h)| *h).collect::<Vec<_>>(),
        vec![second, third]
    );
    assert!(econ.squad(second).await.unwrap().is_empty());
    assert!(econ.end_hires_of(avatar).await.unwrap().is_empty());
    assert_eq!(coin(&econ, second).await, 9_000);
    // A hire runs out after its window: it leaves the squad and frees the slot.
    sqlx::query("update hires set at = now() - interval '13 hours' where hirer_character = $1")
        .bind(hirer)
        .execute(econ.pool())
        .await
        .unwrap();
    assert!(econ.squad(hirer).await.unwrap().is_empty());
    econ.hire(third, fresh, 1, None, |_| true).await.unwrap();
    sound(&econ).await;
}

/// What Phase 12 ruled of hires and trades (PARTY.md 6, 7): a hire names the price it was
/// shown and buys the build that was listed; a listing is withdrawn; a trade is between
/// two in one zone, is called off when one is claimed elsewhere or after ten idle
/// minutes, and a stranger neither accepts nor cancels it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hire_buys_what_was_listed_and_a_trade_ends_when_its_two_part() {
    let Some(econ) = setup().await else { return };
    let hirer = player(&econ, 10_000).await;
    let (_, avatar) = account_with_character(&econ).await;
    let listed_build: serde_json::Value =
        sqlx::query_scalar("select build from characters where id = $1")
            .bind(avatar)
            .fetch_one(econ.pool())
            .await
            .unwrap();
    econ.hire_list(avatar, 1_000).await.unwrap();
    assert_eq!(econ.hire_listed(avatar).await.unwrap(), Some(1_000));
    // The price shown was another: nothing is hired, nothing moves.
    assert_eq!(
        econ.hire(hirer, avatar, 3, Some(900), |_| true).await,
        Err(EconError::State(PRICE_CHANGED.into()))
    );
    assert_eq!(coin(&econ, hirer).await, 10_000);
    // The owner makes something else of the character after listing it: the listing,
    // the tavern and the hire keep the build that was listed.
    let other_build = serde_json::json!({"not": "what was listed"});
    sqlx::query("update characters set build = $2 where id = $1")
        .bind(avatar)
        .bind(&other_build)
        .execute(econ.pool())
        .await
        .unwrap();
    let row = econ
        .tavern()
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.character == avatar)
        .unwrap();
    assert_eq!(row.build, listed_build);
    // A listing whose build cannot be played sells nothing, and nothing is paid.
    assert_eq!(
        econ.hire(hirer, avatar, 3, Some(1_000), |_| false).await,
        Err(EconError::State(NOT_FOR_HIRE.into()))
    );
    assert_eq!(coin(&econ, hirer).await, 10_000);
    let (hire, _) = econ
        .hire(hirer, avatar, 3, Some(1_000), |b| *b == listed_build)
        .await
        .unwrap();
    let squad = econ.squad(hirer).await.unwrap();
    assert_eq!((squad.len(), squad[0].id), (1, hire));
    assert_eq!(
        squad[0].build, listed_build,
        "the hire keeps the listed build"
    );
    // Withdrawn: not in the tavern, not for hire, and the hire that runs is not ended.
    econ.hire_unlist(avatar).await.unwrap();
    assert_eq!(econ.hire_listed(avatar).await.unwrap(), None);
    assert!(
        !econ
            .tavern()
            .await
            .unwrap()
            .iter()
            .any(|t| t.character == avatar)
    );
    assert_eq!(
        econ.hire(hirer, avatar, 3, Some(1_000), |_| true).await,
        Err(EconError::NotFound)
    );
    assert_eq!(econ.squad(hirer).await.unwrap().len(), 1);
    // A listing made before builds were kept with listings (none) is not served.
    econ.hire_list(avatar, 1_000).await.unwrap();
    sqlx::query("update hire_listings set build = null where character_id = $1")
        .bind(avatar)
        .execute(econ.pool())
        .await
        .unwrap();
    assert!(
        !econ
            .tavern()
            .await
            .unwrap()
            .iter()
            .any(|t| t.character == avatar)
    );
    let second = player(&econ, 10_000).await;
    assert_eq!(
        econ.hire(second, avatar, 3, Some(1_000), |_| true).await,
        Err(EconError::State(NOT_FOR_HIRE.into()))
    );

    // Trades. Two in one zone, a stranger, and the ends a trade comes to.
    let mut strict = econ.clone();
    strict.trades_need_a_zone = true;
    let (a, b, stranger) = (
        player(&econ, 1_000).await,
        player(&econ, 0).await,
        player(&econ, 0).await,
    );
    let in_zone = |who: i64, zone: Option<&'static str>| {
        let pool = econ.pool().clone();
        async move {
            match zone {
                Some(z) => sqlx::query("update characters set location_kind = 'zone', location_zone = $2 where id = $1")
                    .bind(who)
                    .bind(z)
                    .execute(&pool)
                    .await
                    .unwrap(),
                None => sqlx::query("update characters set location_kind = 'offline', location_zone = null where id = $1")
                    .bind(who)
                    .execute(&pool)
                    .await
                    .unwrap(),
            };
        }
    };
    in_zone(a, Some("square")).await;
    in_zone(b, Some("square")).await;
    let t = strict.trade_open_in(a, b, "square").await.unwrap();
    strict.trade_set_coin(t, a, 100).await.unwrap();
    cooldown(&strict).await;
    let v = strict.trade_version(t).await.unwrap();
    // A stranger accepts nothing and changes nothing.
    assert_eq!(
        strict.trade_accept(t, stranger, v).await,
        Err(EconError::Forbidden)
    );
    assert_eq!(
        strict.trade_view(t, a).await.unwrap().state,
        TradeState::Open
    );
    // One of the two is elsewhere: the view says so, and an accept calls it off.
    in_zone(b, None).await;
    assert!(!strict.trade_view(t, a).await.unwrap().together);
    assert_eq!(
        strict.trade_accept(t, a, v).await,
        Err(EconError::State(NOT_HERE.into()))
    );
    assert_eq!(
        strict.trade_view(t, a).await.unwrap().state,
        TradeState::Cancelled
    );
    assert_eq!(coin(&econ, a).await, 1_000);
    // A trade nobody touches for ten minutes is called off by the sweep; one whose
    // character a zone claims is called off at the claim.
    in_zone(b, Some("square")).await;
    let idle = strict.trade_open_in(a, b, "square").await.unwrap();
    sqlx::query("update trades set changed_at = now() - interval '11 minutes' where id = $1")
        .bind(idle)
        .execute(econ.pool())
        .await
        .unwrap();
    assert_eq!(strict.trades_expire().await.unwrap(), 1);
    assert_eq!(
        strict.trade_view(idle, b).await.unwrap().state,
        TradeState::Cancelled
    );
    let claimed = strict.trade_open_in(a, b, "square").await.unwrap();
    assert_eq!(strict.trades_end_of(b).await.unwrap(), 1);
    assert_eq!(
        strict.trade_view(claimed, a).await.unwrap().state,
        TradeState::Cancelled
    );
    assert_eq!(strict.trades_end_of(b).await.unwrap(), 0);
    in_zone(a, None).await;
    in_zone(b, None).await;
    sound(&econ).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn crossing_movements_neither_deadlock_nor_lose_coin() {
    let Some(econ) = setup().await else { return };
    // Two stall owners buy from each other while a third party trades with both: opposite
    // lock orders if the code were careless.
    let a = player(&econ, 10_000).await;
    let b = player(&econ, 10_000).await;
    let sa = econ.stall_open(a, "cross_town", 0, 0).await.unwrap();
    let sb = econ.stall_open(b, "cross_town", 1, 0).await.unwrap();
    let mut tasks = Vec::new();
    for round in 0..10 {
        let ia = component(&econ, a, "gem/quartz").await;
        let ib = component(&econ, b, "gem/quartz").await;
        let la = econ
            .stall_list(a, "cross_town", ia, 100 + round)
            .await
            .unwrap();
        let lb = econ
            .stall_list(b, "cross_town", ib, 200 + round)
            .await
            .unwrap();
        let (e1, e2) = (econ.clone(), econ.clone());
        tasks.push(tokio::spawn(async move {
            e1.stall_buy(b, "cross_town", sa, la, 100 + round).await
        }));
        tasks.push(tokio::spawn(async move {
            e2.stall_buy(a, "cross_town", sb, lb, 200 + round).await
        }));
        let (e3, e4) = (econ.clone(), econ.clone());
        tasks.push(tokio::spawn(async move { e3.grant_coin(a, 1, 0).await }));
        tasks.push(tokio::spawn(async move { e4.grant_coin(b, 1, 0).await }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    // a paid 200..209 and earned 100..109; b the reverse; ten silver granted to each.
    assert_eq!(coin(&econ, a).await, 10_000 - 2_045 + 1_045 + 10);
    assert_eq!(coin(&econ, b).await, 10_000 - 1_045 + 2_045 + 10);
    econ.stall_close(a).await.unwrap();
    econ.stall_close(b).await.unwrap();
    assert_eq!(econ.inventory(a).await.unwrap().1.len(), 10);
    sound(&econ).await;
    let s = econ.supply().await.unwrap();
    assert_eq!(s.created, s.circulating + s.burned);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_storm_of_mixed_movements_never_deadlocks() {
    let Some(econ) = setup().await else { return };
    // Six characters do everything to each other at once: trades, stall sales and closes,
    // buy orders filled and cancelled, crafts of offered items, contracts and drops. Any
    // deadlock would surface as `Busy`; any lost or invented silver fails the audit.
    let mut who = Vec::new();
    for _ in 0..6 {
        who.push(player(&econ, 50_000).await);
    }
    // They play in a zone, so that they can put things on (ITEMS.md 2).
    sqlx::query(
        "update characters set location_kind = 'zone', location_zone = 'storm' where id = any($1)",
    )
    .bind(&who)
    .execute(econ_pool(&econ))
    .await
    .unwrap();
    let items = gm_content::items::load_items(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/content"
    )))
    .expect("items");
    let started = std::time::Instant::now();
    let mut tasks: Vec<tokio::task::JoinHandle<Result<(), EconError>>> = Vec::new();
    let mut ops = 0u32;
    for i in 0..6usize {
        let me = who[i];
        let next = who[(i + 1) % 6];
        let prev = who[(i + 5) % 6];
        let e = econ.clone();
        let content = items.clone();
        ops += 8 * 15;
        // Each character runs eight rounds of its own (sixteen items: the inventory never fills); the six run at once and every round
        // reaches into both neighbours' holders.
        tasks.push(tokio::spawn(async move {
            for round in 0..8i32 {
                let town = format!("storm_{round}");
                // A stall with a listing and a buy order; the neighbour buys and fills.
                let stall = e.stall_open(me, &town, i as i32, 0).await?;
                let item = e
                    .grant_components(&town, &[(me, "gem/quartz".into())], 0)
                    .await?[0];
                let listing = e.stall_list(me, &town, item, 10).await?;
                let order = e.buy_order_post(me, "core/iron", 7, 2).await?;
                let theirs = e
                    .grant_components(&town, &[(next, "core/iron".into())], 0)
                    .await?[0];
                let (e1, e2, e3) = (e.clone(), e.clone(), e.clone());
                let at = town.clone();
                let buy =
                    tokio::spawn(async move { e1.stall_buy(next, &at, stall, listing, 10).await });
                let fill =
                    tokio::spawn(async move { e2.buy_order_fill(next, order, theirs).await });
                // A contract accepted and decided while the coin is moving.
                let contract = e.contract_post(me, &town, 100, 10).await?;
                e.contract_accept(prev, contract, &[prev, next]).await?;
                let report =
                    tokio::spawn(
                        async move { e3.contract_report(contract, Outcome::Completed).await },
                    );
                buy.await.unwrap()?;
                fill.await.unwrap()?;
                report.await.unwrap()?;
                e.buy_order_cancel(me, order).await?;
                e.stall_close(me).await?;
                // The stall returned the delivered core; with a frame it becomes a cuirass,
                // which is decomposed again.
                let frame = e
                    .grant_components(&town, &[(me, "frame/oak".into())], 0)
                    .await?[0];
                let cuirass = e.craft(me, "cuirass", &[theirs, frame], None).await?;
                // Worn, it cannot be taken apart; taken off, it can. (An armour: the
                // weapon's place is being fought over by the sword below.)
                e.wear(me, "storm", cuirass, &content, &[]).await?;
                if e.decompose(me, cuirass).await.is_ok() {
                    return Err(EconError::Invalid("a worn cuirass was decomposed".into()));
                }
                e.take_off(me, "storm", cuirass, &content).await?;
                e.decompose(me, cuirass).await?;
            }
            Ok(())
        }));
        // A sword is put on and taken off while it is being offered in trades to a
        // neighbour who accepts whatever is there: it must never be worn and on offer, and
        // never leave the one who wears it.
        let (e, a, b, content) = (econ.clone(), who[i], who[(i + 2) % 6], items.clone());
        ops += 10 * 4;
        tasks.push(tokio::spawn(async move {
            let parts = ["core/iron".to_string(), "frame/oak".to_string()];
            let sword = e.grant_item(a, "sword", &parts, None).await?;
            for round in 0..10 {
                let t = e.trade_open(a, b).await?;
                let (e1, e2, c1) = (e.clone(), e.clone(), content.clone());
                let offer = tokio::spawn(async move { e1.trade_offer_item(t, a, sword).await });
                let wear = tokio::spawn(async move {
                    e2.wear(a, "storm", sword, &c1, &["sword".to_string()])
                        .await
                });
                let offered = offer.await.unwrap();
                wear.await.unwrap()?;
                // Whichever came first, the sword is worn now and in no offer.
                match offered {
                    Ok(()) | Err(EconError::State(_)) => {}
                    Err(other) => return Err(other),
                }
                let mine = e.trade_view(t, a).await?.mine;
                if !mine.2.is_empty() {
                    return Err(EconError::Invalid(format!(
                        "round {round}: a worn sword is on offer"
                    )));
                }
                e.take_off(a, "storm", sword, &content).await?;
                e.trade_cancel(t, a).await?;
            }
            Ok(())
        }));
        // Meanwhile coin drops and trade offers cross the same holders.
        let (e, a, b) = (econ.clone(), who[i], who[(i + 3) % 6]);
        ops += 12 * 3;
        tasks.push(tokio::spawn(async move {
            for _ in 0..12 {
                e.grant_coin(a, 3, 0).await?;
                let t = e.trade_open(a, b).await?;
                e.trade_set_coin(t, a, 1).await?;
            }
            Ok(())
        }));
    }
    let mut failures = Vec::new();
    for t in tasks {
        if let Err(e) = t.await.unwrap() {
            failures.push(e);
        }
    }
    let secs = started.elapsed().as_secs_f64();
    // A full inventory is a legitimate refusal in this storm; a deadlock or an internal
    // error is not.
    failures.retain(|e| *e != EconError::Full);
    assert!(failures.is_empty(), "{failures:?}");
    println!(
        "storm: {ops} operations in {secs:.2} s ({:.0} per second)",
        ops as f64 / secs
    );
    sound(&econ).await;
    let s = econ.supply().await.unwrap();
    assert_eq!(s.created, s.circulating + s.burned);
}
