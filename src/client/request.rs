/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/
use crate::subscription::Subscription;

pub struct SubscriptionRequest {
    pub(crate) subscription: Option<Subscription>,
    pub(crate) subscription_id: Option<usize>,
}
