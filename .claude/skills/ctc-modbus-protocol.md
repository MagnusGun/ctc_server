# CTC Modbus Protocol Specification

This skill provides the official Modbus protocol specifications for CTC Building Management Systems (BMS) as documented in "Service document-BMS Register-17003548.pdf" (2021-11-08).

## When to Use This Skill

Use this skill when you need information about:
- Modbus communication protocol details for CTC systems
- Function codes and register addressing
- Communication parameters and requirements
- Remote control functions via BMS

**IMPORTANT**: This skill contains specifications from official documentation. Do NOT reference the Rust implementation in this project - it may contain mistakes. Always trust the documentation as the source of truth.

## Compatibility Note

Not all CTC models support all features documented here. In particular:
- **Register 1100 (Virtual Digital Inputs)**: Not supported on some EcoHeat 400 firmware versions
- For SmartGrid control on unsupported models, use physical GPIO terminals K24/K25 instead

## Modbus Protocol Specifications

### Supported Interfaces

**BMS over Modbus (RS485)**
- Display software versions: 2017-01-01 to 2020-02-19

**BMS over Modbus (TCP)**
- Display software versions: 2020-09-17 or later

### Register Addressing

- **Register numbers start above standard Modbus**: Larger than 49999
- **Offset**: 0
- **Max registers per transmission**: 100

### Function Codes

**Reading**
- Register type: Analog Output Holding Registers
- Function code: 0x03H / 3

**Writing**
- Register type: Analog Output Holding Registers
- Function code: 0x10H / 16 (Write Multiple Registers)

### Communication Requirements

**Update Rate**
- Minimum update interval: 1000ms (1 second) between reads
- For remote control functions: Update value at least every 5 minutes

### Register Categories

The BMS uses several distinct register address ranges:

**Configuration Parameters (61000 series)**
- Read/Write access
- Limited write cycles (flash memory)
- Used for system settings and adjustments

**Status/Reading Parameters (62000 series)**
- Read-Only access
- Current operational values and status information

**Control Parameters (1000 series)**
- Frequent updates allowed (every 5 minutes)
- Remote control functions

**Alarm/Info System (65000 series)**
- Alarm and information text retrieval
- Bit-coded alarm/info buffers

## Remote Control Functions Through BMS

### Update Requirement
**Note**: Update value at least every five minutes for all remote control functions.

### Available Control Registers

**All Controls**

| Function | Address | Description |
|----------|---------|-------------|
| Maximum RPS | 1002 | Maximum revolutions per second for heat pump |
| Maximum power immersion heater lower tank | 1003 | Power limit for lower tank immersion heater |
| Maximum power immersion heater upper tank | 1004 | Power limit for upper tank immersion heater |
| Virtual digital in | 1100¹ | Virtual digital inputs (bit-coded) |

**CTC EcoLogic S Specific**

| Function | Address | Description |
|----------|---------|-------------|
| Start heat pump | 1000 | Command to start heat pump |
| Setpoint heat pump primary flow | 1001 | Target temperature for primary flow |

### Virtual Digital Inputs (Address 1100)

The virtual digital input register uses bit positions to represent different inputs:

| PIN | BIT Position |
|-----|--------------|
| BMS Di 0 | Bit 0 |
| BMS Di 1 | Bit 1 |
| BMS Di 2 | Bit 2 |
| BMS Di 3 | Bit 3 |
| BMS Di 4 | Bit 4 |
| BMS Di 5 | Bit 5 |
| BMS Di 6 | Bit 6 |
| BMS Di 7 | Bit 7 |

## Protocol Usage Guidelines

1. **Register Read Operations**
   - Use function code 0x03 (Read Holding Registers)
   - Maximum 100 registers per transmission
   - Respect 1000ms minimum interval between reads

2. **Register Write Operations**
   - Use function code 0x10 (Write Multiple Registers)
   - For configuration parameters (61000 series): minimize write cycles
   - For control parameters (1000 series): update at least every 5 minutes

3. **Address Format**
   - All register addresses start above 49999
   - Use offset 0 when addressing
   - Example: Register 61500 is addressed as 61500

4. **Communication Best Practices**
   - Poll critical registers at reasonable intervals (e.g., every 5 seconds)
   - Batch register reads when possible (up to 100 registers)
   - For remote control: maintain 5-minute update heartbeat

## Compatible CTC Products

This Modbus protocol is supported by the following CTC product families:
- EcoHeat 400
- EcoZenith i250/i350/i550
- CTC GSi/GS
- EcoLogic/EcoLogic S

## Document Reference

**Source**: Service document-BMS Register-17003548.pdf
**Date**: 2021-11-08
**Pages**: 1-2, 16 (Modbus specifications and remote control functions)
