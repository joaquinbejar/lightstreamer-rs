// Copyright (C) 2024 Joaquín Béjar García
// Portions of this file are derived from lightstreamer-client
// Copyright (C) 2024 Daniel López Azaña
// Original project: https://github.com/daniloaz/lightstreamer-client
//
// This file is part of lightstreamer-rs.
//
// lightstreamer-rs is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// lightstreamer-rs is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with lightstreamer-rs. If not, see <https://www.gnu.org/licenses/>.

use crate::subscription::SubscriptionListener;
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use tokio::sync::mpsc::{Receiver, Sender, channel};

/// Enum representing the snapshot delivery preferences to be requested to Lightstreamer Server for the items in the Subscription.
#[derive(Debug, Default, Clone, Copy, Hash)]
pub enum Snapshot {
    /// Request the full snapshot for the subscribed items.
    Yes,
    /// Do not request any snapshot for the subscribed items.
    No,
    /// Request a snapshot with a specific length (number of updates).
    Number(usize),
    /// Default value. No snapshot preference will be sent to the server.
    #[default]
    None,
}

impl Default for &Snapshot {
    fn default() -> Self {
        &Snapshot::None
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Snapshot::Yes => write!(f, "true"),
            Snapshot::No => write!(f, "false"),
            Snapshot::Number(n) => write!(f, "{}", n),
            Snapshot::None => write!(f, ""),
        }
    }
}

/// Enum representing the subscription mode.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum SubscriptionMode {
    /// MERGE mode. The server sends an update for a specific item only if the state of at least one of the fields has changed.
    Merge,
    /// DISTINCT mode. The server sends an update for an item every time new data for that item is available.
    Distinct,
    /// RAW mode. The server forwards any update for an item that it receives, without performing any processing.
    Raw,
    /// COMMAND mode. The server sends updates based on add, update, and delete commands.
    Command,
}

impl fmt::Display for SubscriptionMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SubscriptionMode::Merge => write!(f, "MERGE"),
            SubscriptionMode::Distinct => write!(f, "DISTINCT"),
            SubscriptionMode::Command => write!(f, "COMMAND"),
            SubscriptionMode::Raw => write!(f, "RAW"),
        }
    }
}

/// Struct representing a Subscription to be submitted to a Lightstreamer Server.
/// It contains subscription details and the listeners needed to process the real-time data.
pub struct Subscription {
    /// The subscription mode for the items, required by Lightstreamer Server.
    mode: SubscriptionMode,
    /// An array of items to be subscribed to through Lightstreamer server.
    items: Option<Vec<String>>,
    /// An "Item Group" identifier representing a list of items.
    item_group: Option<String>,
    /// An array of fields for the items to be subscribed to through Lightstreamer Server.
    fields: Option<Vec<String>>,
    /// A "Field Schema" identifier representing a list of fields.
    field_schema: Option<String>,
    /// The name of the Data Adapter that supplies all the items for this Subscription.
    data_adapter: Option<String>,
    /// The name of the second-level Data Adapter for a COMMAND Subscription.
    command_second_level_data_adapter: Option<String>,
    /// The "Field List" to be subscribed to through Lightstreamer Server for the second-level items in a COMMAND Subscription.
    command_second_level_fields: Option<Vec<String>>,
    /// The "Field Schema" to be subscribed to through Lightstreamer Server for the second-level items in a COMMAND Subscription.
    command_second_level_field_schema: Option<String>,
    /// The length to be requested to Lightstreamer Server for the internal queuing buffers for the items in the Subscription.
    requested_buffer_size: Option<usize>,
    /// The maximum update frequency to be requested to Lightstreamer Server for all the items in the Subscription.
    requested_max_frequency: Option<f64>,
    /// The snapshot delivery preferences to be requested to Lightstreamer Server for the items in the Subscription.
    requested_snapshot: Option<Snapshot>,
    /// The selector name for all the items in the Subscription, used as a filter on the updates received.
    selector: Option<String>,
    /// A list of SubscriptionListener instances that will receive events from this Subscription.
    listeners: Vec<Box<dyn SubscriptionListener>>,
    /// A HashMap storing the latest values received for each item/field pair.
    values: HashMap<(usize, usize), String>,
    /// A HashMap storing the latest values received for each key/field pair in a COMMAND Subscription.
    command_values: HashMap<String, HashMap<usize, String>>,
    /// A flag indicating whether the Subscription is currently active or not.
    is_active: bool,
    /// A flag indicating whether the Subscription is currently subscribed to through the server or not.
    is_subscribed: bool,
    /// Client assigned subscription ID.
    pub(crate) id: usize,
    /// A channel sender to send the subscription ID to the Lightstreamer client.
    pub(crate) id_sender: Sender<usize>,
    /// A channel receiver to receive the subscription ID from the Lightstreamer client.
    pub(crate) id_receiver: Receiver<usize>,
}

impl Subscription {
    /// Constructor for creating a new Subscription instance.
    ///
    /// # Parameters
    /// - `mode`: The subscription mode for the items, required by Lightstreamer Server.
    /// - `items`: An array of items to be subscribed to through Lightstreamer server. It is also possible to specify the "Item List" or "Item Group" later.
    /// - `fields`: An array of fields for the items to be subscribed to through Lightstreamer Server. It is also possible to specify the "Field List" or "Field Schema" later.
    ///
    /// # Errors
    /// Returns an error if no items or fields are provided.
    pub fn new(
        mode: SubscriptionMode,
        items: Option<Vec<String>>,
        fields: Option<Vec<String>>,
    ) -> Result<Subscription, Box<dyn std::error::Error>> {
        if items.is_none() || fields.is_none() {
            return Err("Items and fields must be provided".to_string().into());
        }

        let (id_sender, id_receiver) = channel(1);

        Ok(Subscription {
            mode,
            items,
            item_group: None,
            fields,
            field_schema: None,
            data_adapter: None,
            command_second_level_data_adapter: None,
            command_second_level_fields: None,
            command_second_level_field_schema: None,
            requested_buffer_size: None,
            requested_max_frequency: None,
            requested_snapshot: None,
            selector: None,
            listeners: Vec::new(),
            values: HashMap::new(),
            command_values: HashMap::new(),
            is_active: false,
            is_subscribed: false,
            id: 0,
            id_sender,
            id_receiver,
        })
    }

    /// Adds a listener that will receive events from the Subscription instance.
    ///
    /// The same listener can be added to several different Subscription instances.
    ///
    /// # Lifecycle
    /// A listener can be added at any time. A call to add a listener already present will be ignored.
    ///
    /// # Parameters
    /// - `listener`: An object that will receive the events as documented in the SubscriptionListener interface.
    ///
    /// # See also
    /// `removeListener()`
    pub fn add_listener(&mut self, listener: Box<dyn SubscriptionListener>) {
        self.listeners.push(listener);
    }

    /// Removes a listener from the Subscription instance so that it will not receive events anymore.
    ///
    /// # Lifecycle
    /// A listener can be removed at any time.
    ///
    /// # Parameters
    /// - `listener`: The listener to be removed.
    ///
    /// # See also
    /// `addListener()`
    pub fn remove_listener<T>(&mut self, listener: &T)
    where
        T: SubscriptionListener,
    {
        self.listeners.retain(|l| {
            let l_ref = l.as_ref() as &dyn SubscriptionListener;
            let listener_ref = listener as &dyn SubscriptionListener;
            std::ptr::addr_of!(*l_ref) != std::ptr::addr_of!(*listener_ref)
        });
    }

    /// Returns a list containing the SubscriptionListener instances that were added to this client.
    ///
    /// # Returns
    /// A list containing the listeners that were added to this client.
    ///
    /// # See also
    /// `addListener()`
    pub fn get_listeners(&self) -> &Vec<Box<dyn SubscriptionListener>> {
        &self.listeners
    }

    /// Inquiry method that can be used to read the mode specified for this Subscription.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// The Subscription mode specified in the constructor.
    pub fn get_mode(&self) -> &SubscriptionMode {
        &self.mode
    }

    /// Setter method that sets the "Item Group" to be subscribed to through Lightstreamer Server.
    ///
    /// Any call to this method will override any "Item List" or "Item Group" previously specified.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// Returns an error if the Subscription is currently "active".
    ///
    /// # Parameters
    /// - `group`: A String to be expanded into an item list by the Metadata Adapter.
    pub fn set_item_group(&mut self, group: String) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active. This method can only be called while the Subscription instance is in its 'inactive' state.".to_string());
        }
        self.item_group = Some(group);
        Ok(())
    }

    /// Inquiry method that can be used to read the item group specified for this Subscription.
    ///
    /// # Lifecycle
    /// This method can only be called if the Subscription has been initialized using an "Item Group"
    ///
    /// # Returns
    /// The "Item Group" to be subscribed to through the server, or `None` if the Subscription was initialized with an "Item List" or was not initialized at all.
    pub fn get_item_group(&self) -> Option<&String> {
        self.item_group.as_ref()
    }

    /// Setter method that sets the "Item List" to be subscribed to through Lightstreamer Server.
    ///
    /// Any call to this method will override any "Item List" or "Item Group" previously specified.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if any of the item names in the "Item List" contains a space, is a number, or is empty/None.
    ///
    /// # Parameters
    /// - `items`: An array of items to be subscribed to through the server.
    pub fn set_items(&mut self, items: Vec<String>) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        for item in &items {
            if item.contains(" ") || item.parse::<usize>().is_ok() || item.is_empty() {
                return Err("Invalid item name".to_string());
            }
        }
        self.items = Some(items);
        Ok(())
    }

    /// Inquiry method that can be used to read the "Item List" specified for this Subscription.
    /// Note that if the single-item-constructor was used, this method will return an array of length 1 containing such item.
    ///
    /// # Lifecycle
    /// This method can only be called if the Subscription has been initialized with an "Item List".
    ///
    /// # Returns
    /// The "Item List" to be subscribed to through the server, or `None` if the Subscription was initialized with an "Item Group" or was not initialized at all.
    pub fn get_items(&self) -> Option<&Vec<String>> {
        self.items.as_ref()
    }

    /// Setter method that sets the "Field Schema" to be subscribed to through Lightstreamer Server.
    ///
    /// Any call to this method will override any "Field List" or "Field Schema" previously specified.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// Returns an error if the Subscription is currently "active".
    ///
    /// # Parameters
    /// - `schema`: A String to be expanded into a field list by the Metadata Adapter.
    pub fn set_field_schema(&mut self, schema: String) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        self.field_schema = Some(schema);
        Ok(())
    }

    /// Inquiry method that can be used to read the field schema specified for this Subscription.
    ///
    /// # Lifecycle
    /// This method can only be called if the Subscription has been initialized using a "Field Schema"
    ///
    /// # Returns
    /// The "Field Schema" to be subscribed to through the server, or `None` if the Subscription was initialized with a "Field List" or was not initialized at all.
    pub fn get_field_schema(&self) -> Option<&String> {
        self.field_schema.as_ref()
    }

    /// Setter method that sets the "Field List" to be subscribed to through Lightstreamer Server.
    ///
    /// Any call to this method will override any "Field List" or "Field Schema" previously specified.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if any of the field names in the list contains a space or is empty/None.
    ///
    /// # Parameters
    /// - `fields`: An array of fields to be subscribed to through the server.
    pub fn set_fields(&mut self, fields: Vec<String>) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        for field in &fields {
            if field.contains(" ") || field.is_empty() {
                return Err("Invalid field name".to_string());
            }
        }
        self.fields = Some(fields);
        Ok(())
    }

    /// Inquiry method that can be used to read the "Field List" specified for this Subscription.
    ///
    /// # Lifecycle
    /// This method can only be called if the Subscription has been initialized using a "Field List".
    ///
    /// # Returns
    /// The "Field List" to be subscribed to through the server, or `None` if the Subscription was initialized with a "Field Schema" or was not initialized at all.
    pub fn get_fields(&self) -> Option<&Vec<String>> {
        self.fields.as_ref()
    }

    /// Setter method that sets the name of the Data Adapter (within the Adapter Set used by the current session) that supplies all the items for this Subscription.
    ///
    /// The Data Adapter name is configured on the server side through the "name" attribute of the `<data_provider>` element, in the "adapters.xml" file that defines the Adapter Set (a missing attribute configures the "DEFAULT" name).
    ///
    /// Note that if more than one Data Adapter is needed to supply all the items in a set of items, then it is not possible to group all the items of the set in a single Subscription. Multiple Subscriptions have to be defined.
    ///
    /// # Default
    /// The default Data Adapter for the Adapter Set, configured as "DEFAULT" on the Server.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// Returns an error if the Subscription is currently "active".
    ///
    /// # Parameters
    /// - `adapter`: The name of the Data Adapter. A `None` value is equivalent to the "DEFAULT" name.
    ///
    /// # See also
    /// `ConnectionDetails.setAdapterSet()`
    pub fn set_data_adapter(&mut self, adapter: Option<String>) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        self.data_adapter = adapter;
        Ok(())
    }

    /// Inquiry method that can be used to read the name of the Data Adapter specified for this Subscription through `setDataAdapter()`.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// The name of the Data Adapter; returns `None` if no name has been configured, so that the "DEFAULT" Adapter Set is used.
    pub fn get_data_adapter(&self) -> Option<&String> {
        self.data_adapter.as_ref()
    }

    /// Setter method that sets the name of the second-level Data Adapter (within the Adapter Set used by the current session)
    /// Setter method that sets the name of the second-level Data Adapter (within the Adapter Set used by the current session) that supplies all the second-level items for a COMMAND Subscription.
    ///
    /// All the possible second-level items should be supplied in "MERGE" mode with snapshot available.
    ///
    /// The Data Adapter name is configured on the server side through the "name" attribute of the `<data_provider>` element, in the "adapters.xml" file that defines the Adapter Set (a missing attribute configures the "DEFAULT" name).
    ///
    /// # Default
    /// The default Data Adapter for the Adapter Set, configured as "DEFAULT" on the Server.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if the Subscription mode is not "COMMAND".
    ///
    /// # Parameters
    /// - `adapter`: The name of the Data Adapter. A `None` value is equivalent to the "DEFAULT" name.
    ///
    /// # See also
    /// `Subscription.setCommandSecondLevelFields()`
    ///
    /// # See also
    /// `Subscription.setCommandSecondLevelFieldSchema()`
    pub fn set_command_second_level_data_adapter(
        &mut self,
        adapter: Option<String>,
    ) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        if self.mode != SubscriptionMode::Command {
            return Err("Subscription mode is not Command".to_string());
        }
        self.command_second_level_data_adapter = adapter;
        Ok(())
    }

    /// Inquiry method that can be used to read the name of the second-level Data Adapter specified for this Subscription through `setCommandSecondLevelDataAdapter()`.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Errors
    /// Returns an error if the Subscription mode is not COMMAND.
    ///
    /// # Returns
    /// The name of the second-level Data Adapter.
    ///
    /// # See also
    /// `setCommandSecondLevelDataAdapter()`
    pub fn get_command_second_level_data_adapter(&self) -> Option<&String> {
        if self.mode != SubscriptionMode::Command {
            return None;
        }
        self.command_second_level_data_adapter.as_ref()
    }

    /// Setter method that sets the "Field Schema" to be subscribed to through Lightstreamer Server for the second-level items. It can only be used on COMMAND Subscriptions.
    ///
    /// Any call to this method will override any "Field List" or "Field Schema" previously specified for the second-level.
    ///
    /// Calling this method enables the two-level behavior:
    ///
    /// In synthesis, each time a new key is received on the COMMAND Subscription, the key value is treated as an Item name and an underlying Subscription for this Item is created and subscribed to automatically, to feed fields specified by this method. This mono-item Subscription is specified through an "Item List" containing only the Item name received. As a consequence, all the conditions provided for subscriptions through Item Lists have to be satisfied. The item is subscribed to in "MERGE" mode, with snapshot request and with the same maximum frequency setting as for the first-level items (including the "unfiltered" case). All other Subscription properties are left as the default. When the key is deleted by a DELETE command on the first-level Subscription, the associated second-level Subscription is also unsubscribed from.
    ///
    /// Specify `None` as parameter will disable the two-level behavior.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if the Subscription mode is not "COMMAND".
    ///
    /// # Parameters
    /// - `schema`: A String to be expanded into a field list by the Metadata Adapter.
    ///
    /// # See also
    /// `Subscription.setCommandSecondLevelFields()`
    pub fn set_command_second_level_field_schema(
        &mut self,
        schema: Option<String>,
    ) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        if self.mode != SubscriptionMode::Command {
            return Err("Subscription mode is not Command".to_string());
        }
        self.command_second_level_field_schema = schema;
        Ok(())
    }

    /// Inquiry method that can be used to read the "Field Schema" specified for second-level Subscriptions.
    ///
    /// # Lifecycle
    /// This method can only be called if the second-level of this Subscription has been initialized using a "Field Schema".
    ///
    /// # Errors
    /// Returns an error if the Subscription mode is not COMMAND.
    ///
    /// # Returns
    /// The "Field Schema" to be subscribed to through the server, or `None` if the Subscription was initialized with a "Field List" or was not initialized at all.
    ///
    /// # See also
    /// `Subscription.setCommandSecondLevelFieldSchema()`
    pub fn get_command_second_level_field_schema(&self) -> Option<&String> {
        if self.mode != SubscriptionMode::Command {
            return None;
        }
        self.command_second_level_field_schema.as_ref()
    }

    /// Setter method that sets the "Field List" to be subscribed to through Lightstreamer Server for the second-level items. It can only be used on COMMAND Subscriptions.
    ///
    /// Any call to this method will override any "Field List" or "Field Schema" previously specified for the second-level.
    ///
    /// Calling this method enables the two-level behavior:
    ///
    /// In synthesis, each time a new key is received on the COMMAND Subscription, the key value is treated as an Item name and an underlying Subscription for this Item is created and subscribed to automatically, to feed fields specified by this method. This mono-item Subscription is specified through an "Item List" containing only the Item name received. As a consequence, all the conditions provided for subscriptions through Item Lists have to be satisfied. The item is subscribed to in "MERGE" mode, with snapshot request and with the same maximum frequency setting as for the first-level items (including the "unfiltered" case). All other Subscription properties are left as the default. When the key is deleted by a DELETE command on the first-level Subscription, the associated second-level Subscription is also unsubscribed from.
    ///
    /// Specifying `None` as parameter will disable the two-level behavior.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if the Subscription mode is not "COMMAND".
    /// - Returns an error if any of the field names in the "Field List" contains a space or is empty/None.
    ///
    /// # Parameters
    /// - `fields`: An array of Strings containing a list of fields to be subscribed to through the server. Ensure that no name conflict is generated between first-level and second-level fields. In case of conflict, the second-level field will not be accessible by name, but only by position.
    ///
    /// # See also
    /// `Subscription.setCommandSecondLevelFieldSchema()`
    pub fn set_command_second_level_fields(
        &mut self,
        fields: Option<Vec<String>>,
    ) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        if self.mode != SubscriptionMode::Command {
            return Err("Subscription mode is not Command".to_string());
        }
        if let Some(ref fields) = fields {
            for field in fields {
                if field.contains(" ") || field.is_empty() {
                    return Err("Invalid field name".to_string());
                }
            }
        }
        self.command_second_level_fields = fields;
        Ok(())
    }

    /// Inquiry method that can be used to read the "Field List" specified for second-level Subscriptions.
    ///
    /// # Lifecycle
    /// This method can only be called if the second-level of this Subscription has been initialized using a "Field List"
    ///
    /// # Errors
    /// Returns an error if the Subscription mode is not COMMAND.
    ///
    /// # Returns
    /// The list of fields to be subscribed to through the server, or `None` if the Subscription was initialized with a "Field Schema" or was not initialized at all.
    ///
    /// # See also
    /// `Subscription.setCommandSecondLevelFields()`
    pub fn get_command_second_level_fields(&self) -> Option<&Vec<String>> {
        if self.mode != SubscriptionMode::Command {
            return None;
        }
        self.command_second_level_fields.as_ref()
    }

    /// Setter method that sets the length to be requested to Lightstreamer Server for the internal queuing buffers for the items in the Subscription. A Queuing buffer is used by the Server to accumulate a burst of updates for an item, so that they can all be sent to the client, despite of bandwidth or frequency limits. It can be used only when the subscription mode is MERGE or DISTINCT and unfiltered dispatching has not been requested. Note that the Server may pose an upper limit on the size of its internal buffers.
    ///
    /// # Default
    /// `None`, meaning to lean on the Server default based on the subscription mode. This means that the buffer size will be 1 for MERGE subscriptions and "unlimited" for DISTINCT subscriptions. See the "General Concepts" document for further details.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if the specified value is not `None` nor "unlimited" nor a valid positive integer number.
    ///
    /// # Parameters
    /// - `size`: An integer number, representing the length of the internal queuing buffers to be used in the Server. If the string "unlimited" is supplied, then no buffer size limit is requested (the check is case insensitive). It is also possible to supply a `None` value to stick to the Server default (which currently depends on the subscription mode).
    ///
    /// # See also
    /// `Subscription.setRequestedMaxFrequency()`
    pub fn set_requested_buffer_size(&mut self, size: Option<usize>) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        self.requested_buffer_size = size;
        Ok(())
    }

    /// Inquiry method that can be used to read the buffer size, configured though `setRequestedBufferSize()`, to be requested to the Server for this Subscription.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// An integer number, representing the buffer size to be requested to the server, or the string "unlimited", or `None`.
    pub fn get_requested_buffer_size(&self) -> Option<&usize> {
        self.requested_buffer_size.as_ref()
    }

    /// Setter method that sets the maximum update frequency to be requested to Lightstreamer Server for all the items in the Subscription. It can be used only if the Subscription mode is MERGE, DISTINCT or COMMAND (in the latter case, the frequency limitation applies to the UPDATE events for each single key). For Subscriptions with two-level behavior (see `Subscription.setCommandSecondLevelFields()` and `Subscription.setCommandSecondLevelFieldSchema()`), the specified frequency limit applies to both first-level and second-level items.
    ///
    /// Note that frequency limits on the items can also be set on the server side and this request can only be issued in order to furtherly reduce the frequency, not to rise it beyond these limits.
    ///
    /// This method can also be used to request unfiltered dispatching for the items in the Subscription. However, unfiltered dispatching requests may be refused if any frequency limit is posed on the server side for some item.
    ///
    /// # General Edition Note
    /// A further global frequency limit could also be imposed by the Server, depending on Edition and License Type; this specific limit also applies to RAW mode and to unfiltered dispatching. To know what features are enabled by your license, please see the License tab of the Monitoring Dashboard (by default, available at /dashboard).
    ///
    /// # Default
    /// `None`, meaning to lean on the Server default based on the subscription mode. This consists, for all modes, in not applying any frequency limit to the subscription (the same as "unlimited"); see the "General Concepts" document for further details.
    ///
    /// # Lifecycle
    /// This method can can be called at any time with some differences based on the Subscription status:
    ///
    /// - If the Subscription instance is in its "inactive" state then this method can be called at will.
    ///
    /// - If the Subscription instance is in its "active" state then the method can still be called unless the current value is "unfiltered" or the supplied value is "unfiltered" or `None`. If the Subscription instance is in its "active" state and the connection to the server is currently open, then a request to change the frequency of the Subscription on the fly is sent to the server.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active" and the current value of this property is "unfiltered".
    /// - Returns an error if the Subscription is currently "active" and the given parameter is `None` or "unfiltered".
    /// - Returns an error if the specified value is not `None` nor one of the special "unlimited" and "unfiltered" values nor a valid positive number.
    ///
    /// # Parameters
    /// - `freq`: A decimal number, representing the maximum update frequency (expressed in updates per second) for each item in the Subscription; for instance, with a setting of 0.5, for each single item, no more than one update every 2 seconds will be received. If the string "unlimited" is supplied, then no frequency limit is requested. It is also possible to supply the string "unfiltered", to ask for unfiltered dispatching, if it is allowed for the items, or a `None` value to stick to the Server default (which currently corresponds to "unlimited"). The check for the string constants is case insensitive.
    pub fn set_requested_max_frequency(&mut self, freq: Option<f64>) -> Result<(), String> {
        if self.is_active && self.requested_max_frequency.is_none() {
            return Err("Subscription is active and current value is unfiltered".to_string());
        }
        if self.is_active && freq.is_none() {
            return Err("Cannot set unfiltered while active".to_string());
        }
        if self.is_active && freq.is_none() {
            return Err("Cannot set None while active".to_string());
        }
        self.requested_max_frequency = freq;
        Ok(())
    }

    /// Inquiry method that can be used to read the max frequency, configured through `setRequestedMaxFrequency()`, to be requested to the Server for this Subscription.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// A decimal number, representing the max frequency to be requested to the server (expressed in updates per second), or the strings "unlimited" or "unfiltered", or `None`.
    pub fn get_requested_max_frequency(&self) -> Option<&f64> {
        self.requested_max_frequency.as_ref()
    }

    /// Setter method that enables/disables snapshot delivery request for the items in the Subscription. The snapshot can be requested only if the Subscription mode is MERGE, DISTINCT or COMMAND.
    ///
    /// # Default
    /// "yes" if the Subscription mode is not "RAW", `None` otherwise.
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// - Returns an error if the Subscription is currently "active".
    /// - Returns an error if the specified value is not "yes" nor "no" nor `None` nor a valid integer positive number.
    /// - Returns an error if the specified value is not compatible with the mode of the Subscription:
    ///     - In case of a RAW Subscription only `None` is a valid value;
    ///     - In case of a non-DISTINCT Subscription only `None` "yes" and "no" are valid values.
    ///
    /// # Parameters
    /// - `snapshot`: "yes"/"no" to request/not request snapshot delivery (the check is case insensitive). If the Subscription mode is DISTINCT, instead of "yes", it is also possible to supply an integer number, to specify the requested length of the snapshot (though the length of the received snapshot may be less than
    ///   requested, because of insufficient data or server side limits); passing "yes" means that the snapshot length should be determined only by the Server. `None` is also a valid value; if specified, no snapshot preference will be sent to the server that will decide itself whether or not to send any snapshot.
    ///
    /// # See also
    /// `ItemUpdate.isSnapshot()`
    pub fn set_requested_snapshot(&mut self, snapshot: Option<Snapshot>) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        match snapshot {
            Some(Snapshot::None) => {
                if self.mode == SubscriptionMode::Raw {
                    return Err("Cannot request snapshot for Raw mode".to_string());
                }
            }
            Some(Snapshot::Number(_)) => {
                if self.mode != SubscriptionMode::Distinct {
                    return Err("Cannot specify snapshot length for non-Distinct mode".to_string());
                }
            }
            _ => {}
        }
        self.requested_snapshot = snapshot;
        Ok(())
    }

    /// Inquiry method that can be used to read the snapshot preferences, configured through `setRequestedSnapshot()`, to be requested to the Server for this Subscription.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// "yes", "no", `None`, or an integer number.
    pub fn get_requested_snapshot(&self) -> Option<&Snapshot> {
        self.requested_snapshot.as_ref()
    }

    /// Setter method that sets the selector name for all the items in the Subscription. The selector is a filter on the updates received. It is executed on the Server and implemented by the Metadata Adapter.
    ///
    /// # Default
    /// `None` (no selector).
    ///
    /// # Lifecycle
    /// This method can only be called while the Subscription instance is in its "inactive" state.
    ///
    /// # Errors
    /// Returns an error if the Subscription is currently "active".
    ///
    /// # Parameters
    /// - `selector`: The name of a selector, to be recognized by the Metadata Adapter, or `None` to unset the selector.
    pub fn set_selector(&mut self, selector: Option<String>) -> Result<(), String> {
        if self.is_active {
            return Err("Subscription is active".to_string());
        }
        self.selector = selector;
        Ok(())
    }

    /// Inquiry method that can be used to read the selector name specified for this Subscription through `setSelector()`.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// The name of the selector.
    pub fn get_selector(&self) -> Option<&String> {
        self.selector.as_ref()
    }

    /// Returns the latest value received for the specified item/field pair.
    ///
    /// It is suggested to consume real-time data by implementing and adding a proper SubscriptionListener rather than probing this method. In case of COMMAND Subscriptions, the value returned by this method may be misleading, as in COMMAND mode all the keys received, being part of the same item, will overwrite each other; for COMMAND Subscriptions, use `Subscription.getCommandValue()` instead.
    ///
    /// Note that internal data is cleared when the Subscription is unsubscribed from.
    ///
    /// # Lifecycle
    /// This method can be called at any time; if called to retrieve a value that has not been received yet, then it will return `None`.
    ///
    /// # Errors
    /// Returns an error if an invalid item name or field name is specified or if the specified item position or field position is out of bounds.
    ///
    /// # Parameters
    /// - `item_pos`: A String representing an item in the configured item list or a Number representing the 1-based position of the item in the specified item group. (In case an item list was specified, passing the item position is also possible).
    /// - `field_pos`: A String representing a field in the configured field list or a Number representing the 1-based position of the field in the specified field schema. (In case a field list was specified, passing the field position is also possible).
    ///
    /// # Returns
    /// The current value for the specified field of the specified item(possibly `None`), or `None` if no value has been received yet.
    pub fn get_value(&self, item_pos: usize, field_pos: usize) -> Option<&String> {
        self.values.get(&(item_pos, field_pos))
    }

    /// Returns the latest value received for the specified item/key/field combination in a COMMAND Subscription. This method can only be used if the Subscription mode is COMMAND. Subscriptions with two-level behavior are also supported, hence the specified field can be either a first-level or a second-level one.
    ///
    /// It is suggested to consume real-time data by implementing and adding a proper SubscriptionListener rather than probing this method.
    ///
    /// Note that internal data is cleared when the Subscription is unsubscribed from.
    ///
    /// # Lifecycle
    /// This method can be called at any time; if called to retrieve a value that has not been received yet, then it will return `None`.
    ///
    /// # Errors
    /// - Returns an error if an invalid item name or field name is specified or if the specified item position or field position is out of bounds.
    /// - Returns an error if the Subscription mode is not COMMAND.
    ///
    /// # Parameters
    /// - `item_pos`: A String representing an item in the configured item list or a Number representing the 1-based position of the item in the specified item group. (In case an item list was specified, passing the item position is also possible).
    /// - `key`: A String containing the value of a key received on the COMMAND subscription.
    /// - `field_pos`: A String representing a field in the configured field list or a Number representing the 1-based position of the field in the specified field schema. (In case a field list was specified, passing the field position is also possible).
    ///
    /// # Returns
    /// The current value for the specified field of the specified key within the specified item (possibly `None`), or `None` if the specified key has not been added yet (note that it might have been added and eventually deleted).
    pub fn get_command_value(
        &self,
        item_pos: usize,
        key: &str,
        field_pos: usize,
    ) -> Option<&String> {
        let key = format!("{}_{}", item_pos, key);
        self.command_values
            .get(&key)
            .and_then(|fields| fields.get(&field_pos))
    }

    /// Inquiry method that checks if the Subscription is currently "active" or not. Most of the Subscription properties cannot be modified if a Subscription is "active".
    ///
    /// The status of a Subscription is changed to "active" through the `LightstreamerClient.subscribe()` method and back to "inactive" through the `LightstreamerClient.unsubscribe()` one.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// `true`/`false` if the Subscription is "active" or not.
    ///
    /// # See also
    /// `LightstreamerClient.subscribe()`
    ///
    /// # See also
    /// `LightstreamerClient.unsubscribe()`
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Inquiry method that checks if the Subscription is currently subscribed to through the server or not.
    ///
    /// This flag is switched to true by server sent Subscription events, and back to false in case of client disconnection, `LightstreamerClient.unsubscribe()` calls and server sent unsubscription events.
    ///
    /// # Lifecycle
    /// This method can be called at any time.
    ///
    /// # Returns
    /// `true`/`false` if the Subscription is subscribed to through the server or not.
    pub fn is_subscribed(&self) -> bool {
        self.is_subscribed
    }

    /// Returns the position of the "key" field in a COMMAND Subscription.
    ///
    /// This method can only be used if the Subscription mode is COMMAND and the Subscription was initialized using a "Field Schema".
    ///
    /// # Lifecycle
    /// This method can be called at any time after the first `SubscriptionListener.onSubscription()` event.
    ///
    /// # Errors
    /// - Returns an error if the Subscription mode is not COMMAND or if the `SubscriptionListener.onSubscription()` event for this Subscription was not yet fired.
    /// - Returns an error if a "Field List" was specified.
    ///
    /// # Returns
    /// The 1-based position of the "key" field within the "Field Schema".
    pub fn get_key_position(&self) -> Option<usize> {
        if self.mode != SubscriptionMode::Command || !self.is_subscribed {
            return None;
        }
        if let Some(ref schema) = self.field_schema {
            return schema.split(',').position(|field| field.trim() == "key");
        }
        None
    }

    /// Returns the position of the "command" field in a COMMAND Subscription.
    ///
    /// This method can only be used if the Subscription mode is COMMAND and the Subscription was initialized using a "Field Schema".
    ///
    /// # Lifecycle
    /// This method can be called at any time after the first `SubscriptionListener.onSubscription()` event.
    ///
    /// # Errors
    /// - Returns an error if the Subscription mode is not COMMAND or if the `SubscriptionListener.onSubscription()` event for this Subscription was not yet fired.
    ///
    /// # Returns
    /// The 1-based position of the "command" field within the "Field Schema".
    pub fn get_command_position(&self) -> Option<usize> {
        if self.mode != SubscriptionMode::Command || !self.is_subscribed {
            return None;
        }
        if let Some(ref schema) = self.field_schema {
            return schema
                .split(',')
                .position(|field| field.trim() == "command");
        }
        None
    }

    /*
    /// Handles the subscription event.
    pub fn on_subscription(&mut self) {
        self.is_subscribed = true;
        for listener in &mut self.listeners {
            listener.on_subscription();
        }
    }

    /// Handles the unsubscription event.
    pub fn on_unsubscription(&mut self) {
        self.is_subscribed = false;
        self.values.clear();
        self.command_values.clear();
        for listener in &mut self.listeners {
            listener.on_unsubscription();
        }
    }

    /// Handles an update event for a regular Subscription.
    pub fn on_update(&mut self, item_pos: usize, field_pos: usize, value: String, is_snapshot: bool) {
        self.values.insert((item_pos, field_pos), value.clone());
        for listener in &mut self.listeners {
            listener.on_update(item_pos, field_pos, &value, is_snapshot);
        }
    }

    /// Handles an update event for a COMMAND Subscription.
    pub fn on_command_update(&mut self, key: String, item_pos: usize, field_pos: usize, value: String, is_snapshot: bool) {
        self.command_values
            .entry(key.clone())
            .or_insert_with(HashMap::new)
            .insert(field_pos, value.clone());
        for listener in &mut self.listeners {
            listener.on_command_update(&key, item_pos, field_pos, &value, is_snapshot);
        }
    }
    */
}

impl Debug for Subscription {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("mode", &self.mode)
            .field("item_group", &self.item_group)
            .field("items", &self.items)
            .field("field_schema", &self.field_schema)
            .field("fields", &self.fields)
            .field("data_adapter", &self.data_adapter)
            .field(
                "command_second_level_data_adapter",
                &self.command_second_level_data_adapter,
            )
            .field(
                "command_second_level_field_schema",
                &self.command_second_level_field_schema,
            )
            .field(
                "command_second_level_fields",
                &self.command_second_level_fields,
            )
            .field("requested_buffer_size", &self.requested_buffer_size)
            .field("requested_max_frequency", &self.requested_max_frequency)
            .field("requested_snapshot", &self.requested_snapshot)
            .field("selector", &self.selector)
            .field("is_active", &self.is_active)
            .field("is_subscribed", &self.is_subscribed)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::ItemUpdate;
    use std::sync::{Arc, Mutex};

    struct MockSubscriptionListener {
        subscription_called: Arc<Mutex<bool>>,
        unsubscription_called: Arc<Mutex<bool>>,
        item_update_called: Arc<Mutex<bool>>,
    }

    impl MockSubscriptionListener {
        fn new() -> Self {
            MockSubscriptionListener {
                subscription_called: Arc::new(Mutex::new(false)),
                unsubscription_called: Arc::new(Mutex::new(false)),
                item_update_called: Arc::new(Mutex::new(false)),
            }
        }
    }

    impl SubscriptionListener for MockSubscriptionListener {
        fn on_subscription(&mut self) {
            *self.subscription_called.lock().unwrap() = true;
        }

        fn on_unsubscription(&mut self) {
            *self.unsubscription_called.lock().unwrap() = true;
        }

        fn on_item_update(&self, _update: &ItemUpdate) {
            *self.item_update_called.lock().unwrap() = true;
        }
    }

    #[test]
    fn test_new_subscription() {
        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string(), "item2".to_string()]),
            Some(vec!["field1".to_string(), "field2".to_string()]),
        );

        assert!(subscription.is_ok());
        let subscription = subscription.unwrap();

        assert_eq!(*subscription.get_mode(), SubscriptionMode::Merge);
        assert_eq!(
            subscription.get_items().unwrap(),
            &vec!["item1".to_string(), "item2".to_string()]
        );
        assert_eq!(
            subscription.get_fields().unwrap(),
            &vec!["field1".to_string(), "field2".to_string()]
        );

        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            None,
            Some(vec!["field1".to_string()]),
        );
        assert!(subscription.is_err());

        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            None,
        );
        assert!(subscription.is_err());
    }

    #[test]
    fn test_add_and_remove_listener() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();
        assert_eq!(subscription.get_listeners().len(), 0);

        let listener = Box::new(MockSubscriptionListener::new());
        subscription.add_listener(listener);
        assert_eq!(subscription.get_listeners().len(), 1);

        let listener2 = MockSubscriptionListener::new();
        subscription.remove_listener(&listener2);
        assert_eq!(subscription.get_listeners().len(), 1);

        subscription.add_listener(Box::new(listener2));
        assert_eq!(subscription.get_listeners().len(), 2);
    }

    #[test]
    fn test_set_items() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_items(vec!["new_item1".to_string(), "new_item2".to_string()]);
        assert!(result.is_ok());

        assert_eq!(
            subscription.get_items().unwrap(),
            &vec!["new_item1".to_string(), "new_item2".to_string()]
        );

        subscription.is_active = true;

        let result = subscription.set_items(vec!["another_item".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_fields() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result =
            subscription.set_fields(vec!["new_field1".to_string(), "new_field2".to_string()]);
        assert!(result.is_ok());

        assert_eq!(
            subscription.get_fields().unwrap(),
            &vec!["new_field1".to_string(), "new_field2".to_string()]
        );

        subscription.is_active = true;

        let result = subscription.set_fields(vec!["another_field".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_item_group() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_item_group("group1".to_string());
        assert!(result.is_ok());

        assert_eq!(subscription.get_item_group().unwrap(), "group1");

        subscription.is_active = true;

        let result = subscription.set_item_group("another_group".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_set_field_schema() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_field_schema("schema1".to_string());
        assert!(result.is_ok());

        assert_eq!(subscription.get_field_schema().unwrap(), "schema1");

        subscription.is_active = true;

        let result = subscription.set_field_schema("another_schema".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_set_data_adapter() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_data_adapter(Some("adapter1".to_string()));
        assert!(result.is_ok());
        assert_eq!(subscription.get_data_adapter().unwrap(), "adapter1");

        subscription.is_active = true;

        let result = subscription.set_data_adapter(Some("another_adapter".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_requested_snapshot() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_requested_snapshot(Some(Snapshot::Yes));
        assert!(result.is_ok());

        match subscription.get_requested_snapshot().unwrap() {
            Snapshot::Yes => {} // OK
            _ => panic!("Expected Snapshot::Yes"),
        }

        subscription.is_active = true;

        let result = subscription.set_requested_snapshot(Some(Snapshot::No));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_requested_buffer_size() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_requested_buffer_size(Some(10));
        assert!(result.is_ok());

        assert_eq!(subscription.get_requested_buffer_size().unwrap(), &10);

        subscription.is_active = true;

        let result = subscription.set_requested_buffer_size(Some(20));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_requested_max_frequency() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_requested_max_frequency(Some(10.5));
        assert!(result.is_ok());

        assert_eq!(subscription.get_requested_max_frequency().unwrap(), &10.5);

        subscription.is_active = true;

        let result = subscription.set_requested_max_frequency(Some(20.5));
        assert!(result.is_ok());

        subscription.requested_max_frequency = None;
        let result = subscription.set_requested_max_frequency(Some(30.5));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_selector() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = subscription.set_selector(Some("selector1".to_string()));
        assert!(result.is_ok());

        assert_eq!(subscription.get_selector().unwrap(), "selector1");

        subscription.is_active = true;

        let result = subscription.set_selector(Some("another_selector".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_command_second_level_methods() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result =
            subscription.set_command_second_level_data_adapter(Some("adapter1".to_string()));
        assert!(result.is_ok());
        assert_eq!(
            subscription
                .get_command_second_level_data_adapter()
                .unwrap(),
            "adapter1"
        );

        subscription.is_active = true;

        let result =
            subscription.set_command_second_level_data_adapter(Some("adapter2".to_string()));
        assert!(result.is_err());

        subscription.is_active = false;

        let result = subscription.set_command_second_level_fields(Some(vec![
            "field1".to_string(),
            "field2".to_string(),
        ]));
        assert!(result.is_ok());
        assert_eq!(
            subscription.get_command_second_level_fields().unwrap(),
            &vec!["field1".to_string(), "field2".to_string()]
        );

        let result =
            subscription.set_command_second_level_field_schema(Some("schema1".to_string()));
        assert!(result.is_ok());
        assert_eq!(
            subscription
                .get_command_second_level_field_schema()
                .unwrap(),
            "schema1"
        );

        let mut non_command_subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        let result = non_command_subscription
            .set_command_second_level_data_adapter(Some("adapter1".to_string()));
        assert!(result.is_err());

        let result = non_command_subscription
            .set_command_second_level_fields(Some(vec!["field1".to_string()]));
        assert!(result.is_err());

        let result = non_command_subscription
            .set_command_second_level_field_schema(Some("schema1".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_is_active_and_is_subscribed() {
        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        assert!(!subscription.is_active());
        assert!(!subscription.is_subscribed());
    }

    #[test]
    fn test_get_key_position() {
        // Create a COMMAND subscription with field_schema containing key
        let mut subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec![
                "key".to_string(),
                "command".to_string(),
                "field1".to_string(),
            ]),
        )
        .unwrap();

        // Set field_schema with key field
        subscription.field_schema = Some("key,command,field1".to_string());

        // Not subscribed yet, should return None
        assert_eq!(subscription.get_key_position(), None);

        // Mark as subscribed
        subscription.is_subscribed = true;

        // Now it should return the position of key (0)
        assert_eq!(subscription.get_key_position(), Some(0));

        // Test with a non-COMMAND subscription
        let mut non_command_subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["key".to_string(), "field1".to_string()]),
        )
        .unwrap();

        non_command_subscription.field_schema = Some("key,field1".to_string());
        non_command_subscription.is_subscribed = true;

        // Should return None for non-COMMAND subscription
        assert_eq!(non_command_subscription.get_key_position(), None);

        // Test with COMMAND subscription but no key field
        let mut no_key_subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec!["command".to_string(), "field1".to_string()]),
        )
        .unwrap();

        no_key_subscription.field_schema = Some("command,field1".to_string());
        no_key_subscription.is_subscribed = true;

        // Should return None when key field is not present
        assert_eq!(no_key_subscription.get_key_position(), None);
    }

    #[test]
    fn test_get_command_position() {
        // Create a COMMAND subscription with field_schema containing command
        let mut subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec![
                "key".to_string(),
                "command".to_string(),
                "field1".to_string(),
            ]),
        )
        .unwrap();

        // Set field_schema with command field
        subscription.field_schema = Some("key,command,field1".to_string());

        // Not subscribed yet, should return None
        assert_eq!(subscription.get_command_position(), None);

        // Mark as subscribed
        subscription.is_subscribed = true;

        // Now it should return the position of command (1)
        assert_eq!(subscription.get_command_position(), Some(1));

        // Test with a non-COMMAND subscription
        let mut non_command_subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["command".to_string(), "field1".to_string()]),
        )
        .unwrap();

        non_command_subscription.field_schema = Some("command,field1".to_string());
        non_command_subscription.is_subscribed = true;

        // Should return None for non-COMMAND subscription
        assert_eq!(non_command_subscription.get_command_position(), None);

        // Test with COMMAND subscription but no command field
        let mut no_command_subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec!["key".to_string(), "field1".to_string()]),
        )
        .unwrap();

        no_command_subscription.field_schema = Some("key,field1".to_string());
        no_command_subscription.is_subscribed = true;

        // Should return None when command field is not present
        assert_eq!(no_command_subscription.get_command_position(), None);
    }

    #[test]
    fn test_debug_implementation() {
        let subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        // Test that Debug implementation works without panicking
        let debug_string = format!("{:?}", subscription);

        // Verify it contains some expected fields
        assert!(debug_string.contains("mode"));
        assert!(debug_string.contains("items"));
        assert!(debug_string.contains("fields"));
        assert!(debug_string.contains("is_active"));
        assert!(debug_string.contains("is_subscribed"));
    }

    #[test]
    fn test_snapshot_display() {
        // Test the Display implementation for Snapshot
        assert_eq!(format!("{}", Snapshot::Yes), "true");
        assert_eq!(format!("{}", Snapshot::No), "false");
        assert_eq!(format!("{}", Snapshot::Number(5)), "5");
        assert_eq!(format!("{}", Snapshot::None), "");
    }

    #[test]
    fn test_subscription_mode_display() {
        // Test the Display implementation for SubscriptionMode
        assert_eq!(format!("{}", SubscriptionMode::Merge), "MERGE");
        assert_eq!(format!("{}", SubscriptionMode::Distinct), "DISTINCT");
        assert_eq!(format!("{}", SubscriptionMode::Command), "COMMAND");
        assert_eq!(format!("{}", SubscriptionMode::Raw), "RAW");
    }

    #[test]
    fn test_snapshot_default() {
        // Test the Default implementation for &Snapshot
        let default_snapshot: &Snapshot = Default::default();
        assert!(matches!(default_snapshot, &Snapshot::None));
    }

    #[test]
    fn test_get_command_value() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec![
                "key".to_string(),
                "command".to_string(),
                "field1".to_string(),
            ]),
        )
        .unwrap();

        // Manually add some command values to test get_command_value
        let mut field_map = HashMap::new();
        field_map.insert(3, "test_value".to_string());
        subscription
            .command_values
            .insert("1_test_key".to_string(), field_map);

        // Test getting an existing value
        let value = subscription.get_command_value(1, "test_key", 3);
        assert_eq!(value, Some(&"test_value".to_string()));

        // Test getting a non-existent key
        let value = subscription.get_command_value(1, "non_existent_key", 3);
        assert_eq!(value, None);

        // Test getting a non-existent field position
        let value = subscription.get_command_value(1, "test_key", 4);
        assert_eq!(value, None);

        // Test getting a non-existent item position
        let value = subscription.get_command_value(2, "test_key", 3);
        assert_eq!(value, None);
    }

    #[test]
    fn test_set_requested_buffer_size_with_unlimited() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        // Test setting buffer size to "unlimited"
        let result = subscription.set_requested_buffer_size(None);
        assert!(result.is_ok());
        assert_eq!(subscription.get_requested_buffer_size(), None);
    }

    #[test]
    fn test_command_second_level_field_methods_with_invalid_inputs() {
        // Test with non-COMMAND subscription
        let mut non_command_subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        // Test set_command_second_level_fields with invalid subscription mode
        let result = non_command_subscription
            .set_command_second_level_fields(Some(vec!["field1".to_string()]));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Subscription mode is not Command");

        // Test set_command_second_level_field_schema with invalid subscription mode
        let result = non_command_subscription
            .set_command_second_level_field_schema(Some("field1".to_string()));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Subscription mode is not Command");

        // Test with COMMAND subscription but active
        let mut command_subscription = Subscription::new(
            SubscriptionMode::Command,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        // Make the subscription active
        command_subscription.is_active = true;

        // Test set_command_second_level_fields with active subscription
        let result =
            command_subscription.set_command_second_level_fields(Some(vec!["field1".to_string()]));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Subscription is active");

        // Test set_command_second_level_field_schema with active subscription
        let result =
            command_subscription.set_command_second_level_field_schema(Some("field1".to_string()));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Subscription is active");
    }

    #[test]
    fn test_set_data_adapter_with_invalid_inputs() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        // Make the subscription active
        subscription.is_active = true;

        // Test set_data_adapter with active subscription
        let result = subscription.set_data_adapter(Some("adapter1".to_string()));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Subscription is active");
    }

    #[test]
    fn test_set_selector_with_invalid_inputs() {
        let mut subscription = Subscription::new(
            SubscriptionMode::Merge,
            Some(vec!["item1".to_string()]),
            Some(vec!["field1".to_string()]),
        )
        .unwrap();

        // Make the subscription active
        subscription.is_active = true;

        // Test set_selector with active subscription
        let result = subscription.set_selector(Some("selector1".to_string()));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Subscription is active");
    }
}
