/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/
use crate::subscription::Subscription;

/// Retrieve a reference to a subscription with the given `id`
pub(crate) fn get_subscription_by_id(
    subscriptions: &[Subscription],
    subscription_id: usize,
) -> Option<&Subscription> {
    subscriptions.iter().find(|sub| sub.id == subscription_id)
}
