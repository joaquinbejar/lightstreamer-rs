/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/
use crate::subscription::Subscription;

/// A request to subscribe or unsubscribe from a Lightstreamer data stream.
///
/// This struct is used internally by the LightstreamerClient to manage subscription
/// and unsubscription operations. It contains either a Subscription object for new
/// subscriptions or a subscription ID for unsubscribing from an existing subscription.
pub struct SubscriptionRequest {
    /// The subscription to be added. Set to None when unsubscribing.
    pub(crate) subscription: Option<Subscription>,
    /// The ID of the subscription to be removed. Set to None when subscribing.
    pub(crate) subscription_id: Option<usize>,
}
