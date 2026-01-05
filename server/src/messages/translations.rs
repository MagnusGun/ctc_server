//! Alarm and info message translations
//!
//! Contains translation tables for alarm codes (E-codes) and info codes (I-codes)
//! from Swedish to English, plus detailed descriptions in both languages.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Alarm/info translation table (Swedish code -> English short description)
pub static ALARM_TRANSLATIONS: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // Info codes
    m.insert("I002", "Heating off, heat circ. 1");
    m.insert("I005", "Heating off, heat circ. 2");
    m.insert("I008", "Tariff, HP off");
    m.insert("I009", "Compressor blocked");
    m.insert("I010", "Tariff, EL off");
    m.insert("I011", "Ripple control");
    m.insert("I012", "High curr., reduced elec.");
    m.insert("I013", "Start delay");
    m.insert("I014", "Drying period active");
    m.insert("I017", "Smart: Block");
    m.insert("I018", "Smart: Over capacity");
    m.insert("I019", "Smart: Low price");
    m.insert("I021", "Ext. Ctrl Heating 1");
    m.insert("I022", "Ext. Ctrl Heating 2");
    m.insert("I028", "Holiday period");
    m.insert("I030", "Driver block undervoltage");
    m.insert("I031", "Driver block alarm");
    // Alarm codes
    m.insert("E003", "Sensor brine in");
    m.insert("E005", "Sensor brine out");
    m.insert("E010", "Compressor type?");
    m.insert("E013", "EVO off");
    m.insert("E021", "Ext. Motor protection");
    m.insert("E024", "Fuse blown");
    m.insert("E026", "Heat pump");
    m.insert("E027", "Communication error HP");
    m.insert("E028", "Sensor HPin");
    m.insert("E029", "Sensor HPout");
    m.insert("E030", "Sensor outdoor");
    m.insert("E031", "Sensor prim flow 1");
    m.insert("E032", "Sensor prim flow 2");
    m.insert("E035", "High pressure switch");
    m.insert("E036", "Sensor high pressure");
    m.insert("E037", "Sensor discharge");
    m.insert("E040", "Low brine flow");
    m.insert("E041", "Low brine temp");
    m.insert("E043", "Sensor low pressure");
    m.insert("E044", "Stop, high compr temp");
    m.insert("E045", "Stop, low evaporation");
    m.insert("E046", "Stop, high evaporation");
    m.insert("E047", "Stop, low suct gas exp.v.");
    m.insert("E048", "Stop, low evapor exp.v.");
    m.insert("E049", "Stop, high evapor exp.v.");
    m.insert("E050", "Stop, low superheat exp.v");
    m.insert("E052", "Phase 1 missing");
    m.insert("E053", "Phase 2 missing");
    m.insert("E054", "Phase 3 missing");
    m.insert("E055", "Wrong phase order");
    m.insert("E057", "Motor protect high curr.");
    m.insert("E058", "Motor protect low curr.");
    m.insert("E061", "Max thermostat");
    m.insert("E063", "Comm. err. relay board");
    m.insert("E074", "Room sensor 1");
    m.insert("E075", "Room sensor 2");
    m.insert("E080", "Sensor suction gas");
    m.insert("E086", "Comm error EXPANSION");
    m.insert("E137", "Sensor Diffthermostat");
    m.insert("E138", "Sensor EcoTank bottom");
    m.insert("E139", "Sensor EcoTank top");
    m
});

/// Detailed descriptions for alarm/info codes (English)
pub static ALARM_DESCRIPTIONS: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // Info descriptions
    m.insert(
        "I002",
        "Heating function for heat circuit 1 is currently disabled",
    );
    m.insert(
        "I005",
        "Heating function for heat circuit 2 is currently disabled",
    );
    m.insert(
        "I008",
        "Heat pump is blocked due to tariff/time control settings",
    );
    m.insert(
        "I009",
        "Compressor is temporarily blocked (normal operation)",
    );
    m.insert(
        "I010",
        "Electric heater is blocked due to tariff/time control settings",
    );
    m.insert("I011", "Ripple control signal received from utility");
    m.insert("I012", "High current detected, electric heating reduced");
    m.insert(
        "I013",
        "Compressor start delay active - prevents rapid cycling",
    );
    m.insert(
        "I014",
        "Floor drying program is active - elevated temperatures for concrete curing",
    );
    m.insert(
        "I017",
        "SmartGrid blocking mode - heating reduced due to grid signal or high prices",
    );
    m.insert(
        "I018",
        "SmartGrid overcapacity mode - maximizing heat production during low prices",
    );
    m.insert(
        "I019",
        "SmartGrid low price mode - prioritizing heat pump during favorable rates",
    );
    m.insert(
        "I021",
        "External control signal is controlling heat circuit 1",
    );
    m.insert(
        "I022",
        "External control signal is controlling heat circuit 2",
    );
    m.insert(
        "I028",
        "Holiday mode is active - reduced heating during absence",
    );
    m.insert("I030", "Driver blocked due to undervoltage condition");
    m.insert("I031", "Driver blocked due to alarm condition");
    // Alarm descriptions
    m.insert(
        "E003",
        "Brine inlet temperature sensor fault - check wiring or replace sensor",
    );
    m.insert(
        "E005",
        "Brine outlet temperature sensor fault - check wiring or replace sensor",
    );
    m.insert(
        "E010",
        "Compressor type not recognized - check configuration",
    );
    m.insert("E013", "EVO unit is off or not responding");
    m.insert("E021", "External motor protection triggered");
    m.insert("E024", "Fuse has blown - check and replace fuse");
    m.insert("E026", "Heat pump general fault");
    m.insert(
        "E027",
        "Communication lost with heat pump module - check connections",
    );
    m.insert("E028", "Heat pump inlet temperature sensor fault");
    m.insert("E029", "Heat pump outlet temperature sensor fault");
    m.insert("E030", "Outdoor temperature sensor fault - check wiring");
    m.insert("E031", "Primary flow temperature sensor 1 fault");
    m.insert("E032", "Primary flow temperature sensor 2 fault");
    m.insert(
        "E035",
        "High pressure switch triggered - system overpressure detected",
    );
    m.insert("E036", "High pressure sensor fault");
    m.insert("E037", "Discharge temperature sensor fault");
    m.insert(
        "E040",
        "Insufficient brine flow - check circulation pump and pipes for blockage",
    );
    m.insert(
        "E041",
        "Brine temperature too low - risk of freezing, check antifreeze level",
    );
    m.insert("E043", "Low pressure sensor fault");
    m.insert(
        "E044",
        "Compressor stopped due to high temperature - check ventilation",
    );
    m.insert(
        "E045",
        "Compressor stopped due to low evaporation temperature",
    );
    m.insert(
        "E046",
        "Compressor stopped due to high evaporation temperature",
    );
    m.insert(
        "E047",
        "Compressor stopped due to low suction gas temperature at expansion valve",
    );
    m.insert(
        "E048",
        "Compressor stopped due to low evaporator temperature at expansion valve",
    );
    m.insert(
        "E049",
        "Compressor stopped due to high evaporator temperature at expansion valve",
    );
    m.insert(
        "E050",
        "Compressor stopped due to low superheat at expansion valve",
    );
    m.insert(
        "E052",
        "Phase 1 power supply missing - check electrical connection",
    );
    m.insert(
        "E053",
        "Phase 2 power supply missing - check electrical connection",
    );
    m.insert(
        "E054",
        "Phase 3 power supply missing - check electrical connection",
    );
    m.insert(
        "E055",
        "Incorrect phase sequence - swap two phases at main connection",
    );
    m.insert("E057", "Motor protection triggered due to high current");
    m.insert("E058", "Motor protection triggered due to low current");
    m.insert(
        "E061",
        "Maximum thermostat triggered - safety limit reached",
    );
    m.insert(
        "E063",
        "Communication error with relay board - check internal connections",
    );
    m.insert("E074", "Room temperature sensor 1 fault");
    m.insert("E075", "Room temperature sensor 2 fault");
    m.insert("E080", "Suction gas temperature sensor fault");
    m.insert("E086", "Communication error with expansion module");
    m.insert("E137", "Differential thermostat sensor fault");
    m.insert("E138", "EcoTank bottom temperature sensor fault");
    m.insert("E139", "EcoTank top temperature sensor fault");
    m
});

/// Detailed descriptions for alarm/info codes (Swedish)
pub static ALARM_DESCRIPTIONS_SV: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    // Info descriptions (Swedish)
    m.insert("I002", "Värmefunktion för värmekrets 1 är avstängd");
    m.insert("I005", "Värmefunktion för värmekrets 2 är avstängd");
    m.insert("I008", "Värmepump blockerad pga tariff/tidstyrning");
    m.insert("I009", "Kompressor tillfälligt blockerad (normal drift)");
    m.insert("I010", "Elpatron blockerad pga tariff/tidstyrning");
    m.insert("I011", "Rundstyrningssignal mottagen från elnätet");
    m.insert("I012", "Hög ström - elvärme reducerad");
    m.insert(
        "I013",
        "Kompressorns startfördröjning aktiv - förhindrar snabb cykling",
    );
    m.insert(
        "I014",
        "Golvtorkningsprogram aktivt - förhöjd temperatur för betong",
    );
    m.insert(
        "I017",
        "SmartGrid blockeringsläge - värme reducerad pga nätsignal eller högt pris",
    );
    m.insert(
        "I018",
        "SmartGrid överkapacitet - maximerar värmeproduktion vid lågt pris",
    );
    m.insert(
        "I019",
        "SmartGrid lågprisläge - prioriterar värmepump vid förmånligt pris",
    );
    m.insert("I021", "Extern styrning kontrollerar värmekrets 1");
    m.insert("I022", "Extern styrning kontrollerar värmekrets 2");
    m.insert(
        "I028",
        "Semesterläge aktivt - reducerad uppvärmning under frånvaro",
    );
    m.insert("I030", "Drivrutin blockerad pga underspänning");
    m.insert("I031", "Drivrutin blockerad pga larmtillstånd");
    // Alarm descriptions (Swedish)
    m.insert(
        "E003",
        "Köldbärargivare in fel - kontrollera kablar eller byt givare",
    );
    m.insert(
        "E005",
        "Köldbärargivare ut fel - kontrollera kablar eller byt givare",
    );
    m.insert("E010", "Kompressortyp okänd - kontrollera konfiguration");
    m.insert("E013", "EVO-enhet avstängd eller svarar inte");
    m.insert("E021", "Extern motorskydd utlöst");
    m.insert("E024", "Säkring utlöst - kontrollera och byt säkring");
    m.insert("E026", "Värmepump allmänt fel");
    m.insert(
        "E027",
        "Kommunikation förlorad med värmepumpmodul - kontrollera anslutningar",
    );
    m.insert("E028", "Värmepump inloppsgivarfel");
    m.insert("E029", "Värmepump utloppsgivarfel");
    m.insert("E030", "Utomhusgivare fel - kontrollera kablar");
    m.insert("E031", "Primär framledningsgivare 1 fel");
    m.insert("E032", "Primär framledningsgivare 2 fel");
    m.insert("E035", "Högtrycksbrytare utlöst - övertryck detekterat");
    m.insert("E036", "Högtrycksgivare fel");
    m.insert("E037", "Hetgasgivare fel");
    m.insert(
        "E040",
        "Otillräckligt köldbärarflöde - kontrollera pump och rör",
    );
    m.insert(
        "E041",
        "Köldbärartemperatur för låg - kontrollera frysskydd",
    );
    m.insert("E043", "Lågtrycksgivare fel");
    m.insert(
        "E044",
        "Kompressor stoppad pga hög temperatur - kontrollera ventilation",
    );
    m.insert("E045", "Kompressor stoppad pga låg förångningstemperatur");
    m.insert("E046", "Kompressor stoppad pga hög förångningstemperatur");
    m.insert(
        "E047",
        "Kompressor stoppad pga låg suggas vid expansionsventil",
    );
    m.insert(
        "E048",
        "Kompressor stoppad pga låg förångare vid expansionsventil",
    );
    m.insert(
        "E049",
        "Kompressor stoppad pga hög förångare vid expansionsventil",
    );
    m.insert(
        "E050",
        "Kompressor stoppad pga låg överhettning vid expansionsventil",
    );
    m.insert("E052", "Fas 1 saknas - kontrollera elanslutning");
    m.insert("E053", "Fas 2 saknas - kontrollera elanslutning");
    m.insert("E054", "Fas 3 saknas - kontrollera elanslutning");
    m.insert("E055", "Fel fasföljd - byt två faser vid huvudanslutning");
    m.insert("E057", "Motorskydd utlöst pga hög ström");
    m.insert("E058", "Motorskydd utlöst pga låg ström");
    m.insert("E061", "Maxtermostat utlöst - säkerhetsgräns nådd");
    m.insert(
        "E063",
        "Kommunikationsfel med reläkort - kontrollera interna anslutningar",
    );
    m.insert("E074", "Rumsgivare 1 fel");
    m.insert("E075", "Rumsgivare 2 fel");
    m.insert("E080", "Suggasgivare fel");
    m.insert("E086", "Kommunikationsfel med expansionsmodul");
    m.insert("E137", "Differenstermostatgivare fel");
    m.insert("E138", "EcoTank bottengivare fel");
    m.insert("E139", "EcoTank toppgivare fel");
    m
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alarm_translations() {
        assert_eq!(ALARM_TRANSLATIONS.get("E040"), Some(&"Low brine flow"));
        assert_eq!(ALARM_TRANSLATIONS.get("I019"), Some(&"Smart: Low price"));
        assert_eq!(ALARM_TRANSLATIONS.get("XXXX"), None);
    }

    #[test]
    fn test_alarm_descriptions() {
        assert_eq!(
            ALARM_DESCRIPTIONS.get("I017"),
            Some(&"SmartGrid blocking mode - heating reduced due to grid signal or high prices")
        );
        assert_eq!(
            ALARM_DESCRIPTIONS.get("E040"),
            Some(&"Insufficient brine flow - check circulation pump and pipes for blockage")
        );
        assert_eq!(ALARM_DESCRIPTIONS.get("XXXX"), None);
    }

    #[test]
    fn test_alarm_descriptions_sv() {
        assert_eq!(
            ALARM_DESCRIPTIONS_SV.get("I017"),
            Some(&"SmartGrid blockeringsläge - värme reducerad pga nätsignal eller högt pris")
        );
        assert_eq!(
            ALARM_DESCRIPTIONS_SV.get("E040"),
            Some(&"Otillräckligt köldbärarflöde - kontrollera pump och rör")
        );
        assert_eq!(ALARM_DESCRIPTIONS_SV.get("XXXX"), None);
    }
}
