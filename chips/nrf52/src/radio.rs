//! Radio driver, Bluetooth Low Energy, nRF52
//!
//! Sending Bluetooth Low Energy advertisement packets with payloads up to 31 bytes
//!
//! Currently all fields in PAYLOAD array are configurable from user-space
//! except the PDU_TYPE.
//!
//! ### Author
//! * Niklas Adolfsson <niklasadolfsson1@gmail.com>
//! * Date: July 18, 2017

use core::cell::Cell;
use kernel;
use kernel::hil::gpio::Pin;
use nrf5x;
use nrf5x::ble_advertising_driver::RadioChannel;
use peripheral_registers;


// used for debugging remove later!!
const TX: usize = 17;
const RX: usize = 18;

// NRF52 Specific Radio Constants
const NRF52_RADIO_PCNF0_S1INCL_MSK: u32 = 0;
const NRF52_RADIO_PCNFO_S1INCL_POS: u32 = 20;
const NRF52_RADIO_PCNF0_PLEN_POS: u32 = 24;
const NRF52_RADIO_PCNF0_PLEN_8BITS: u32 = 0;

static mut PAYLOAD: [u8; nrf5x::constants::RADIO_PAYLOAD_LENGTH] =
    [0x00; nrf5x::constants::RADIO_PAYLOAD_LENGTH];

pub struct Radio {
    regs: *const peripheral_registers::RADIO,
    txpower: Cell<usize>,
    client: Cell<Option<&'static nrf5x::ble_advertising_hil::RxClient>>,
    appid: Cell<Option<kernel::AppId>>,
}

pub static mut RADIO: Radio = Radio::new();


unsafe fn toggle_led(pin: usize) {
    let led = &nrf5x::gpio::PORT[pin];
    led.make_output();
    led.toggle();
}


impl Radio {
    pub const fn new() -> Radio {
        Radio {
            regs: peripheral_registers::RADIO_BASE as *const peripheral_registers::RADIO,
            txpower: Cell::new(0),
            client: Cell::new(None),
            appid: Cell::new(None),
        }
    }

    fn set_crc_config(&self) {
        let regs = unsafe { &*self.regs };
        regs.crccnf.set(nrf5x::constants::RADIO_CRCCNF_SKIPADDR <<
                        nrf5x::constants::RADIO_CRCCNF_SKIPADDR_POS |
                        nrf5x::constants::RADIO_CRCCNF_LEN_3BYTES);
        regs.crcinit.set(nrf5x::constants::RADIO_CRCINIT_BLE);
        regs.crcpoly.set(nrf5x::constants::RADIO_CRCPOLY_BLE);
    }


    // Packet configuration
    // Argument unsed atm
    fn set_packet_config(&self, _: u32) {
        let regs = unsafe { &*self.regs };

        // sets the header of PDU TYPE to 1 byte
        // sets the header length to 1 byte
        regs.pcnf0.set((nrf5x::constants::RADIO_PCNF0_LFLEN_1BYTE <<
                        nrf5x::constants::RADIO_PCNF0_LFLEN_POS) |
                       (nrf5x::constants::RADIO_PCNF0_S0_LEN_1BYTE <<
                        nrf5x::constants::RADIO_PCNF0_S0LEN_POS) |
                       (nrf5x::constants::RADIO_PCNF0_S1_ZERO <<
                        nrf5x::constants::RADIO_PCNF0_S1LEN_POS) |
                       (NRF52_RADIO_PCNF0_S1INCL_MSK << NRF52_RADIO_PCNFO_S1INCL_POS) |
                       (NRF52_RADIO_PCNF0_PLEN_8BITS << NRF52_RADIO_PCNF0_PLEN_POS));

        regs.pcnf1.set((nrf5x::constants::RADIO_PCNF1_WHITEEN_ENABLED <<
                        nrf5x::constants::RADIO_PCNF1_WHITEEN_POS) |
                       (nrf5x::constants::RADIO_PCNF1_ENDIAN_LITTLE <<
                        nrf5x::constants::RADIO_PCNF1_ENDIAN_POS) |
                       (nrf5x::constants::RADIO_PCNF1_BALEN_3BYTES <<
                        nrf5x::constants::RADIO_PCNF1_BALEN_POS) |
                       (nrf5x::constants::RADIO_PCNF1_STATLEN_DONT_EXTEND <<
                        nrf5x::constants::RADIO_PCNF1_STATLEN_POS) |
                       (nrf5x::constants::RADIO_PCNF1_MAXLEN_37BYTES <<
                        nrf5x::constants::RADIO_PCNF1_MAXLEN_POS));
    }

    // TODO set from capsules?!
    fn set_rx_address(&self, _: u32) {
        let regs = unsafe { &*self.regs };
        regs.rxaddresses.set(0x01);
    }

    // TODO set from capsules?!
    fn set_tx_address(&self, _: u32) {
        let regs = unsafe { &*self.regs };
        regs.txaddress.set(0x00);
    }

    // should not be configured from the capsule i.e.
    // assume always BLE
    fn set_channel_rate(&self, rate: u32) {
        let regs = unsafe { &*self.regs };
        // set channel rate,  3 - BLE 1MBIT/s
        regs.mode.set(rate);
    }

    fn set_datawhiteiv(&self, val: u32) {
        let regs = unsafe { &*self.regs };
        regs.datawhiteiv.set(val);
    }

    fn set_channel_freq(&self, val: u32) {
        let regs = unsafe { &*self.regs };
        //37, 38 and 39 for adv.
        match val {
            37 => regs.frequency.set(nrf5x::constants::RADIO_FREQ_CH_37),
            38 => regs.frequency.set(nrf5x::constants::RADIO_FREQ_CH_38),
            39 => regs.frequency.set(nrf5x::constants::RADIO_FREQ_CH_39),
            _ => regs.frequency.set(nrf5x::constants::RADIO_FREQ_CH_37),
        }
    }

    fn radio_on(&self) {
        let regs = unsafe { &*self.regs };
        // reset and enable power
        regs.power.set(0);
        regs.power.set(1);
    }

    fn radio_off(&self) {
        let regs = unsafe { &*self.regs };
        regs.power.set(0);
    }

    // pre-condition validated by the capsule before arriving here
    fn set_txpower(&self) {
        let regs = unsafe { &*self.regs };
        regs.txpower.set(self.txpower.get() as u32);
    }

    fn set_buffer(&self) {
        let regs = unsafe { &*self.regs };
        unsafe {
            regs.packetptr.set((&PAYLOAD as *const u8) as u32);
        }
    }


    #[inline(never)]
    pub fn handle_interrupt(&self) {
        let regs = unsafe { &*self.regs };
        self.disable_all_interrupts();

        if regs.event_ready.get() == 1 {
            regs.event_ready.set(0);
            regs.event_end.set(0);
            regs.task_start.set(1);
        }

        if regs.event_address.get() == 1 {
            regs.event_address.set(0);
        }
        if regs.event_payload.get() == 1 {
            regs.event_payload.set(0);
        }

        if regs.event_end.get() == 1 {
            regs.event_end.set(0);
            // this state only verifies that END is received in TX-mode
            // which means that the transmission is finished
            match regs.state.get() {
                nrf5x::constants::RADIO_STATE_TXRU |
                nrf5x::constants::RADIO_STATE_TXIDLE |
                nrf5x::constants::RADIO_STATE_TXDISABLE |
                nrf5x::constants::RADIO_STATE_TX => {
                    self.radio_off();
                    self.client.get().map(|client| {
                        client.advertisement_fired(self.appid
                            .get()
                            .unwrap_or(kernel::AppId::new(0xff)))
                    });
                }
                nrf5x::constants::RADIO_STATE_RXRU |
                nrf5x::constants::RADIO_STATE_RXIDLE |
                nrf5x::constants::RADIO_STATE_RXDISABLE |
                nrf5x::constants::RADIO_STATE_RX => {
                    if regs.crcstatus.get() == 1 {
                        self.radio_off();
                        unsafe {
                            self.client.get().map(|client| {
                                client.receive(&mut PAYLOAD,
                                               PAYLOAD[1] + 1,
                                               kernel::returncode::ReturnCode::SUCCESS,
                                               self.appid.get().unwrap_or(kernel::AppId::new(0xff)))
                            });
                        }
                    } else {
                        self.radio_off();
                        unsafe {
                            self.client.get().map(|client| {
                                client.receive(&mut PAYLOAD,
                                               PAYLOAD[1] + 1,
                                               kernel::returncode::ReturnCode::FAIL,
                                               self.appid.get().unwrap_or(kernel::AppId::new(0xff)))
                            });
                        }
                    }
                }
                // Radio state - Disabled
                _ => (),
            }
        }
        self.enable_interrupts();
    }

    pub fn enable_interrupts(&self) {
        let regs = unsafe { &*self.regs };
        regs.intenset
            .set(nrf5x::constants::RADIO_INTENSET_READY | nrf5x::constants::RADIO_INTENSET_ADDRESS |
                 nrf5x::constants::RADIO_INTENSET_PAYLOAD |
                 nrf5x::constants::RADIO_INTENSET_END);
    }

    pub fn enable_interrupt(&self, intr: u32) {
        let regs = unsafe { &*self.regs };
        regs.intenset.set(intr);
    }

    pub fn clear_interrupt(&self, intr: u32) {
        let regs = unsafe { &*self.regs };
        regs.intenclr.set(intr);
    }

    pub fn disable_all_interrupts(&self) {
        let regs = unsafe { &*self.regs };
        // disable all possible interrupts
        regs.intenclr.set(0xffffffff);
    }

    pub fn reset_payload(&self) {}
}

impl nrf5x::ble_advertising_hil::BleAdvertisementDriver for Radio {
    fn set_advertisement_data(&self, buf: &'static mut [u8], len: usize) -> &'static mut [u8] {
        // set payload
        for (i, c) in buf.as_ref()[0..len].iter().enumerate() {
            unsafe {
                PAYLOAD[i] = *c;
            }
        }
        buf
    }

    fn set_advertisement_txpower(&self, power: usize) -> kernel::ReturnCode {
        match power {
            // +4 dBm, 0 dBm, -4 dBm, -8 dBm, -12 dBm, -16 dBm, -20 dBm, -30 dBm
            0x04 | 0x00 | 0xF4 | 0xFC | 0xF8 | 0xF0 | 0xEC | 0xD8 => {
                self.txpower.set(power);
                kernel::ReturnCode::SUCCESS
            }
            _ => kernel::ReturnCode::ENOSUPPORT,
        }
    }

    #[inline(never)]
    #[no_mangle]
    fn start_advertisement_tx(&self, appid: kernel::AppId, freq: RadioChannel) {
        self.appid.set(Some(appid));


        let regs = unsafe { &*self.regs };

        unsafe {
            toggle_led(TX);
        }

        self.radio_on();

        // TX Power acc. to twpower variable in the struct
        self.set_txpower();

        // BLE MODE
        self.set_channel_rate(nrf5x::constants::RadioMode::Ble1Mbit as u32);

        self.set_channel_freq(freq as u32);
        self.set_datawhiteiv(freq as u32);

        // Set PREFIX | BASE Address
        regs.prefix0.set(0x0000008e);
        regs.base0.set(0x89bed600);

        self.set_tx_address(0x00);
        self.set_rx_address(0x01);
        // regs.RXMATCH.set(0x00);

        // Set Packet Config
        self.set_packet_config(0x00);

        // CRC Config
        self.set_crc_config();

        // Buffer configuration
        self.set_buffer();

        regs.event_ready.set(0);
        regs.task_txen.set(1);

        self.enable_interrupts();
    }

    fn start_advertisement_rx(&self, appid: kernel::AppId, freq: RadioChannel) {
        self.appid.set(Some(appid));

        let regs = unsafe { &*self.regs };

        unsafe {
            toggle_led(RX);
        }

        self.radio_on();

        // BLE MODE
        self.set_channel_rate(nrf5x::constants::RADIO_MODE_BLE_1MBIT);

        self.set_channel_freq(freq as u32);
        self.set_datawhiteiv(freq as u32);

        // Set PREFIX | BASE Address
        regs.prefix0.set(0x0000008e);
        regs.base0.set(0x89bed600);

        self.set_tx_address(0x00);
        self.set_rx_address(0x01);
        // regs.RXMATCH.set(0x00);

        // Set Packet Config
        self.set_packet_config(0x00);

        // CRC Config
        self.set_crc_config();

        // Buffer configuration
        self.set_buffer();

        self.enable_interrupts();

        regs.event_ready.set(0);
        regs.task_rxen.set(1);
    }

    fn set_client(&self, client: &'static nrf5x::ble_advertising_hil::RxClient) {
        self.client.set(Some(client));
    }
}
