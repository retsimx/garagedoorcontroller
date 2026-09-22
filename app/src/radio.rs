//! CYW43 PIO/SPI Radio Bring-Up and Embassy Networking Stack Initialization.

use cyw43::aligned_bytes;
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, PIN_23, PIN_24, PIN_25, PIN_29, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::{bind_interrupts, dma, Peri};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>;
});

/// Shared mutex-guarded CYW43 control handle.
pub type ControlMutex = Mutex<CriticalSectionRawMutex, cyw43::Control<'static>>;

/// Running embassy-net network stack handle.
pub type NetStack = embassy_net::Stack<'static>;

/// Peripherals required to initialize CYW43 and the network stack.
pub struct RadioPeripherals {
    pub pwr: Peri<'static, PIN_23>,
    pub cs: Peri<'static, PIN_25>,
    pub dio: Peri<'static, PIN_24>,
    pub clk: Peri<'static, PIN_29>,
    pub pio: Peri<'static, PIO0>,
    pub dma_ch0: Peri<'static, DMA_CH0>,
    pub dma_ch1: Peri<'static, DMA_CH1>,
}

#[embassy_executor::task]
pub async fn cyw43_task(
    runner: cyw43::Runner<
        'static,
        cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>,
        cyw43::Cyw43439,
    >,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
pub async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

/// Initialize the embassy-net network stack with DHCPv4 and spawn its background runner.
fn init_net_stack(spawner: Spawner, net_device: cyw43::NetDriver<'static>) -> NetStack {
    let mut rng = RoscRng;
    let seed = rng.next_u64();

    let net_config = embassy_net::Config::dhcpv4(Default::default());
    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let (stack, net_runner) = embassy_net::new(
        net_device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(defmt::unwrap!(net_task(net_runner)));
    stack
}

/// Bring up CYW43 radio hardware, spawn background drivers, and initialize the network stack.
pub async fn init(spawner: Spawner, p: RadioPeripherals) -> (&'static ControlMutex, NetStack) {
    let fw = aligned_bytes!("../firmware/43439A0.bin");
    let clm = aligned_bytes!("../firmware/43439A0_clm.bin");
    let nvram = aligned_bytes!("../firmware/nvram_rp2040.bin");

    let pwr = Output::new(p.pwr, Level::Low);
    let cs = Output::new(p.cs, Level::High);
    let mut pio = Pio::new(p.pio, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.dio,
        p.clk,
        dma::Channel::new(p.dma_ch0, Irqs),
        dma::Channel::new(p.dma_ch1, Irqs),
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw, nvram).await;
    spawner.spawn(defmt::unwrap!(cyw43_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::Performance)
        .await;

    let stack = init_net_stack(spawner, net_device);

    static CONTROL_MUTEX: StaticCell<ControlMutex> = StaticCell::new();
    let control_mutex = CONTROL_MUTEX.init(Mutex::new(control));

    (control_mutex, stack)
}
