use quick_xml::Reader;
use quick_xml::events::Event;
use uuid::Uuid;

pub const CIM_ASSOCIATED_POWER_MANAGEMENT_SERVICE: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_AssociatedPowerManagementService";
pub const CIM_POWER_MANAGEMENT_SERVICE: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_PowerManagementService";
pub const CIM_BOOT_SERVICE: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_BootService";
pub const CIM_COMPUTER_SYSTEM: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ComputerSystem";
pub const CIM_COMPUTER_SYSTEM_PACKAGE: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ComputerSystemPackage";
pub const CIM_BOOT_CONFIG_SETTING: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_BootConfigSetting";
pub const CIM_BOOT_SOURCE_SETTING: &str =
    "http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_BootSourceSetting";

const SOAP_GET: &str = "http://schemas.xmlsoap.org/ws/2004/09/transfer/Get";
const WSA_ANONYMOUS: &str = "http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerState {
    On,
    Sleep,
    Reboot,
    Hibernate,
    Off,
    Reset,
}

impl PowerState {
    pub fn code(self) -> u8 {
        match self {
            Self::On => 2,
            Self::Sleep => 4,
            Self::Reboot => 5,
            Self::Hibernate => 7,
            Self::Off => 8,
            Self::Reset => 10,
        }
    }

    pub fn friendly(code: u8) -> &'static str {
        match code {
            2 => "on",
            4 => "sleep",
            5 => "reboot",
            7 => "hibernate",
            8 => "off",
            10 => "reset",
            _ => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootDevice {
    Pxe,
    HardDrive,
    Cd,
}

impl BootDevice {
    pub fn instance_id(self) -> &'static str {
        match self {
            Self::Pxe => "Intel(r) AMT: Force PXE Boot",
            Self::HardDrive => "Intel(r) AMT: Force Hard-drive Boot",
            Self::Cd => "Intel(r) AMT: Force CD/DVD Boot",
        }
    }
}

pub fn get_request(to: &str, resource: &str) -> String {
    envelope(to, SOAP_GET, resource, "", None, None)
}

pub fn power_state_request(to: &str, state: PowerState) -> String {
    let action = format!("{CIM_POWER_MANAGEMENT_SERVICE}/RequestPowerStateChange");
    let body = format!(
        r#"<n1:RequestPowerStateChange_INPUT>
<n1:PowerState>{}</n1:PowerState>
<n1:ManagedElement>
<wsa:Address>{WSA_ANONYMOUS}</wsa:Address>
<wsa:ReferenceParameters>
<wsman:ResourceURI>{CIM_COMPUTER_SYSTEM}</wsman:ResourceURI>
<wsman:SelectorSet>
<wsman:Selector wsman:Name="Name">ManagedSystem</wsman:Selector>
</wsman:SelectorSet>
</wsa:ReferenceParameters>
</n1:ManagedElement>
</n1:RequestPowerStateChange_INPUT>"#,
        state.code()
    );

    envelope(
        to,
        &action,
        CIM_POWER_MANAGEMENT_SERVICE,
        &body,
        Some(("Name", "Intel(r) AMT Power Management Service")),
        Some(("n1", CIM_POWER_MANAGEMENT_SERVICE)),
    )
}

pub fn change_boot_order_request(to: &str, device: BootDevice) -> String {
    let action = format!("{CIM_BOOT_CONFIG_SETTING}/ChangeBootOrder");
    let body = format!(
        r#"<n1:ChangeBootOrder_INPUT>
<n1:Source>
<wsa:Address>{WSA_ANONYMOUS}</wsa:Address>
<wsa:ReferenceParameters>
<wsman:ResourceURI>{CIM_BOOT_SOURCE_SETTING}</wsman:ResourceURI>
<wsman:SelectorSet>
<wsman:Selector wsman:Name="InstanceID">{}</wsman:Selector>
</wsman:SelectorSet>
</wsa:ReferenceParameters>
</n1:Source>
</n1:ChangeBootOrder_INPUT>"#,
        device.instance_id()
    );

    envelope(
        to,
        &action,
        CIM_BOOT_CONFIG_SETTING,
        &body,
        Some(("InstanceID", "Intel(r) AMT: Boot Configuration 0")),
        Some(("n1", CIM_BOOT_CONFIG_SETTING)),
    )
}

pub fn enable_boot_config_request(to: &str) -> String {
    let action = format!("{CIM_BOOT_SERVICE}/SetBootConfigRole");
    let body = format!(
        r#"<n1:SetBootConfigRole_INPUT>
<n1:BootConfigSetting>
<wsa:Address>{WSA_ANONYMOUS}</wsa:Address>
<wsa:ReferenceParameters>
<wsman:ResourceURI>{CIM_BOOT_CONFIG_SETTING}</wsman:ResourceURI>
<wsman:SelectorSet>
<wsman:Selector wsman:Name="InstanceID">Intel(r) AMT: Boot Configuration 0</wsman:Selector>
</wsman:SelectorSet>
</wsa:ReferenceParameters>
</n1:BootConfigSetting>
<n1:Role>1</n1:Role>
</n1:SetBootConfigRole_INPUT>"#
    );

    envelope(
        to,
        &action,
        CIM_BOOT_SERVICE,
        &body,
        Some(("Name", "Intel(r) AMT Boot Service")),
        Some(("n1", CIM_BOOT_SERVICE)),
    )
}

pub fn extract_text(xml: &str, local_name: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if local(event.name().as_ref()) == local_name.as_bytes() => {
                return reader
                    .read_text(event.name())
                    .ok()
                    .map(|text| text.into_owned());
            }
            Ok(Event::Eof) => return None,
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

pub fn return_value(xml: &str) -> Option<u32> {
    extract_text(xml, "ReturnValue").and_then(|value| value.parse().ok())
}

fn local(bytes: &[u8]) -> &[u8] {
    bytes
        .rsplit(|byte| *byte == b':' || *byte == b'}')
        .next()
        .unwrap_or(bytes)
}

fn envelope(
    to: &str,
    action: &str,
    resource: &str,
    body: &str,
    selector: Option<(&str, &str)>,
    body_ns: Option<(&str, &str)>,
) -> String {
    let (body_prefix, body_uri) = body_ns.unwrap_or(("", ""));
    let body_ns_attr = if body_prefix.is_empty() {
        String::new()
    } else {
        format!(r#" xmlns:{body_prefix}="{body_uri}""#)
    };
    let selector = selector.map_or_else(String::new, |(name, value)| {
        format!(
            r#"<wsman:SelectorSet><wsman:Selector Name="{name}">{value}</wsman:Selector></wsman:SelectorSet>"#
        )
    });
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:wsman="http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd"{body_ns_attr}>
<s:Header>
<wsa:Action s:mustUnderstand="true">{action}</wsa:Action>
<wsa:To s:mustUnderstand="true">{to}</wsa:To>
<wsman:ResourceURI s:mustUnderstand="true">{resource}</wsman:ResourceURI>
<wsa:MessageID s:mustUnderstand="true">uuid:{}</wsa:MessageID>
<wsa:ReplyTo><wsa:Address>{WSA_ANONYMOUS}</wsa:Address></wsa:ReplyTo>
{selector}
</s:Header>
<s:Body>{body}</s:Body>
</s:Envelope>"#,
        Uuid::new_v4()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_state_codes_include_modern_reset() {
        assert_eq!(PowerState::On.code(), 2);
        assert_eq!(PowerState::Hibernate.code(), 7);
        assert_eq!(PowerState::Off.code(), 8);
        assert_eq!(PowerState::Reset.code(), 10);
        assert_eq!(PowerState::friendly(10), "reset");
    }

    #[test]
    fn power_request_uses_wsman_path_not_full_uri() {
        let xml = power_state_request("/wsman", PowerState::On);
        assert!(xml.contains("<wsa:To s:mustUnderstand=\"true\">/wsman</wsa:To>"));
        assert!(xml.contains("<n1:PowerState>2</n1:PowerState>"));
        assert!(xml.contains(CIM_POWER_MANAGEMENT_SERVICE));
    }

    #[test]
    fn boot_request_targets_requested_device() {
        let xml = change_boot_order_request("/wsman", BootDevice::Pxe);
        assert!(xml.contains("Intel(r) AMT: Boot Configuration 0"));
        assert!(xml.contains("Intel(r) AMT: Force PXE Boot"));
    }

    #[test]
    fn extracts_namespaced_text() {
        let xml = r#"<s:Envelope xmlns:s="s"><s:Body><n1:ReturnValue xmlns:n1="n">0</n1:ReturnValue><n1:PowerState xmlns:n1="n">8</n1:PowerState></s:Body></s:Envelope>"#;
        assert_eq!(return_value(xml), Some(0));
        assert_eq!(extract_text(xml, "PowerState").as_deref(), Some("8"));
    }
}
