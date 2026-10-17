// A private, client-only i2pd child. Its supervisor owns stdin for its lifetime.
// No service installation, desktop window, public admin API, or console logs.
#include <array>
#include <chrono>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <streambuf>
#include <string>
#include <thread>
#include <openssl/crypto.h>
#include "Daemon.h"
#include "ClientContext.h"
#include "SOCKS.h"
#include "Config.h"

std::string mukhomor_i2p_token;
class NullBuffer final : public std::streambuf {
    int overflow(int c) override { return traits_type::not_eof(c); }
};

int main(int argc, char** argv) {
    try {
        // Fixed-size secret over an anonymous pipe, never argv or environment.
        std::array<char, 65> frame{};
        std::cin.read(frame.data(), frame.size());
        if (std::cin.gcount() != frame.size() || frame[64] != '\n') return 1;
        mukhomor_i2p_token.assign(frame.data(), 64);
        if (mukhomor_i2p_token.find_first_not_of("0123456789abcdef") != std::string::npos) return 1;
        std::thread([] { char byte; std::cin.get(byte); std::_Exit(0); }).detach();
        // Do not load machine/user OpenSSL configuration or external providers.
        if (!OPENSSL_init_crypto(OPENSSL_INIT_NO_LOAD_CONFIG, nullptr)) return 1;
        NullBuffer sink;
        auto log = std::make_shared<std::ostream>(&sink);
        if (!Daemon.init(argc, argv, log)) return 1;
        if (!Daemon.start()) return 1;
        auto proxy = std::dynamic_pointer_cast<const i2p::proxy::SOCKSProxy>(i2p::client::context.GetSocksProxy());
        if (!proxy || !proxy->GetLocalEndpoint().address().is_loopback()) return 1;
        std::cout << "{\"started\":true,\"port\":" << proxy->GetLocalEndpoint().port() << "}" << std::endl;
        bool ready = false;
        for (;;) {
            auto destination = proxy->GetLocalDestination();
            if (!ready && destination && destination->IsReady()) {
                std::cout << "{\"ready\":true}" << std::endl;
                ready = true;
            }
            std::this_thread::sleep_for(std::chrono::seconds(1));
        }
    } catch (...) {
        std::cerr << "I2P_START_FAILED\n";
        return 1;
    }
}
