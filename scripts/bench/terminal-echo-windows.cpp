// Standalone Windows Graphics Capture probe. Build from an x64 MSVC shell.
#define NOMINMAX
#include <windows.h>
#include <d3d11.h>
#include <windows.graphics.capture.interop.h>
#include <windows.graphics.directx.direct3d11.interop.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Graphics.Capture.h>
#include <winrt/Windows.Graphics.DirectX.Direct3D11.h>

#include <algorithm>
#include <array>
#include <bit>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <set>
#include <stdexcept>
#include <string>

namespace capture = winrt::Windows::Graphics::Capture;
namespace graphics = winrt::Windows::Graphics;
namespace directx = winrt::Windows::Graphics::DirectX;

void require(bool condition, const char* message) {
    if (!condition) throw std::runtime_error(message);
}

double qpc_ms() {
    static const double ticks_per_ms = [] {
        LARGE_INTEGER frequency{};
        require(QueryPerformanceFrequency(&frequency), "QueryPerformanceFrequency failed");
        return frequency.QuadPart / 1000.0;
    }();
    LARGE_INTEGER now{};
    require(QueryPerformanceCounter(&now), "QueryPerformanceCounter failed");
    return now.QuadPart / ticks_per_ms;
}

void foreground(HWND window, DWORD pid) {
    DWORD owner = 0;
    require(GetWindowThreadProcessId(window, &owner) && owner == pid &&
            IsWindowVisible(window) && !IsIconic(window) && GetForegroundWindow() == window,
            "Expected the isolated Lyrel HWND in the foreground");
    for (const int key : std::array<int, 5>{VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN}) {
        require(!(GetAsyncKeyState(key) & 0x8000), "A key is held, aborting input");
    }
    require(!(GetKeyState(VK_CAPITAL) & 1), "Turn Caps Lock off before sampling");
}

constexpr std::size_t columns = 8;
constexpr char alphabet[] = ">abcd";
using Row = std::array<std::uint64_t, columns>;
using Templates = std::array<Row, 5>;

std::string token(unsigned sample, char key) {
    std::string result = ">aaaa>a>";
    for (int i = 4; i >= 1; --i) {
        result[i] = 'a' + static_cast<char>(sample % 4);
        sample /= 4;
    }
    result[6] = key;
    return result;
}

std::string recognize(const Row& row, const Templates& patterns) {
    std::string text;
    for (std::size_t column = 0; column < row.size(); ++column) {
        int best = 65;
        int second = 65;
        char glyph = '\0';
        for (std::size_t i = 0; i < patterns.size(); ++i) {
            const int distance = std::popcount(row[column] ^ patterns[i][column]);
            if (distance < best) {
                second = best;
                best = distance;
                glyph = alphabet[i];
            } else {
                second = std::min(second, distance);
            }
        }
        if (best > 3 || second - best < 2) return {};
        const bool delimiter = column == 0 || column == 5 || column == 7;
        if (delimiter != (glyph == '>')) return {};
        text += glyph;
    }
    return text;
}

struct Grid {
    UINT left, top, rows;
    double cell_width, cell_height;

    UINT width() const { return static_cast<UINT>(std::ceil(cell_width * columns)); }
    UINT height() const { return static_cast<UINT>(std::ceil(cell_height * rows)); }

    Row fingerprints(const D3D11_MAPPED_SUBRESOURCE& mapped, UINT row) const {
        Row result{};
        for (std::size_t column = 0; column < result.size(); ++column) {
            for (unsigned y = 0; y < 8; ++y) {
                const auto y0 = static_cast<UINT>((row + y / 8.0) * cell_height);
                const auto y1 = static_cast<UINT>((row + (y + 1) / 8.0) * cell_height);
                for (unsigned x = 0; x < 8; ++x) {
                    const auto x0 = static_cast<UINT>((column + x / 8.0) * cell_width);
                    const auto x1 = static_cast<UINT>((column + (x + 1) / 8.0) * cell_width);
                    int intensity = 0;
                    for (UINT py = y0; py < y1; ++py) {
                        const auto* pixels = static_cast<const std::uint8_t*>(mapped.pData) +
                            static_cast<std::size_t>(py) * mapped.RowPitch;
                        for (UINT px = x0; px < x1; ++px) intensity += pixels[4 * px + 1];
                    }
                    result[column] <<= 1;
                    if (intensity >= 105 * static_cast<int>((x1 - x0) * (y1 - y0))) {
                        result[column] |= 1;
                    }
                }
            }
        }
        return result;
    }
};

void run(HWND hwnd, DWORD pid, const Grid& grid, UINT samples, UINT interval_ms,
         const std::filesystem::path& destination, const std::filesystem::path& start_file) {
    require(std::filesystem::is_directory(destination), "Create the owned evidence directory first");
    for (const wchar_t* name : {L"inputs.csv", L"frames.csv"}) {
        require(!std::filesystem::exists(destination / name), "Use a fresh evidence directory");
    }
    require(start_file.empty() || !std::filesystem::exists(start_file), "Workload start file already exists");
    std::cout << "Focus the isolated fixture pane within 4 seconds.\n" << std::flush;
    Sleep(4000);
    foreground(hwnd, pid);

    require(capture::GraphicsCaptureSession::IsSupported(), "Windows Graphics Capture unavailable");
    auto interop = winrt::get_activation_factory<capture::GraphicsCaptureItem, IGraphicsCaptureItemInterop>();
    capture::GraphicsCaptureItem item{nullptr};
    winrt::check_hresult(interop->CreateForWindow(hwnd, winrt::guid_of<capture::IGraphicsCaptureItem>(),
                                                winrt::put_abi(item)));
    const auto size = item.Size();
    require(grid.left < static_cast<UINT>(size.Width) && grid.top < static_cast<UINT>(size.Height) &&
            grid.width() <= static_cast<UINT>(size.Width) - grid.left &&
            grid.height() <= static_cast<UINT>(size.Height) - grid.top, "Grid ROI outside capture size");

    winrt::com_ptr<ID3D11Device> device;
    winrt::com_ptr<ID3D11DeviceContext> context;
    winrt::check_hresult(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
        device.put(), nullptr, context.put()));
    winrt::com_ptr<::IInspectable> inspectable;
    winrt::check_hresult(CreateDirect3D11DeviceFromDXGIDevice(device.as<IDXGIDevice>().get(), inspectable.put()));
    const auto wrapped = inspectable.as<directx::Direct3D11::IDirect3DDevice>();
    const auto pixel = directx::DirectXPixelFormat::B8G8R8A8UIntNormalized;
    winrt::handle arrived{CreateEventW(nullptr, FALSE, FALSE, nullptr)};
    require(static_cast<bool>(arrived), "CreateEventW failed");
    // Keep the event alive until after the session, subscription and frame pool are closed.
    auto pool = capture::Direct3D11CaptureFramePool::CreateFreeThreaded(wrapped, pixel, 3, size);
    const HANDLE handle = arrived.get();
    auto subscription = pool.FrameArrived(winrt::auto_revoke, [handle](auto&&, auto&&) { SetEvent(handle); });
    auto session = pool.CreateCaptureSession(item);
    session.IsCursorCaptureEnabled(false);
    session.StartCapture();

    const D3D11_TEXTURE2D_DESC description{grid.width(), grid.height(), 1, 1,
        DXGI_FORMAT_B8G8R8A8_UNORM, {1, 0}, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ, 0};
    winrt::com_ptr<ID3D11Texture2D> staging;
    winrt::check_hresult(device->CreateTexture2D(&description, nullptr, staging.put()));
    const D3D11_BOX box{grid.left, grid.top, 0, grid.left + grid.width(), grid.top + grid.height(), 1};
    auto read_grid = [&](const capture::Direct3D11CaptureFrame& frame, auto observe) {
        const auto content = frame.ContentSize();
        require(content.Width == size.Width && content.Height == size.Height, "Window resized during probe");
        winrt::com_ptr<ID3D11Texture2D> texture;
        auto access = frame.Surface().as<::Windows::Graphics::DirectX::Direct3D11::IDirect3DDxgiInterfaceAccess>();
        winrt::check_hresult(access->GetInterface(winrt::guid_of<ID3D11Texture2D>(), texture.put_void()));
        context->CopySubresourceRegion(staging.get(), 0, 0, 0, 0, texture.get(), 0, &box);
        D3D11_MAPPED_SUBRESOURCE mapped{};
        winrt::check_hresult(context->Map(staging.get(), 0, D3D11_MAP_READ, 0, &mapped));
        observe(mapped);
        context->Unmap(staging.get(), 0);
    };

    require(WaitForSingleObject(handle, 3000) == WAIT_OBJECT_0, "No first HWND capture frame");
    const auto calibration = pool.TryGetNextFrame();
    require(static_cast<bool>(calibration), "No calibration frame");
    Templates patterns{};
    read_grid(calibration, [&](const auto& mapped) {
        for (UINT row = 0; row < patterns.size(); ++row) patterns[row] = grid.fingerprints(mapped, row);
    });
    for (std::size_t column = 0; column < columns; ++column) {
        for (std::size_t a = 0; a < patterns.size(); ++a) {
            require(std::popcount(patterns[a][column]) >= 3 && std::popcount(patterns[a][column]) <= 50,
                    "Calibration glyph missing. Check grid bounds and light-on-dark opaque theme");
            for (std::size_t b = 0; b < a; ++b) {
                require(std::popcount(patterns[a][column] ^ patterns[b][column]) >= 5,
                        "Calibration glyphs indistinguishable. Check exact cell dimensions and font");
            }
        }
    }

    std::ofstream inputs(destination / L"inputs.csv");
    std::ofstream frames(destination / L"frames.csv");
    inputs.exceptions(std::ios::failbit | std::ios::badbit);
    frames.exceptions(std::ios::failbit | std::ios::badbit);
    inputs << "id,qpc_ms\n" << std::fixed << std::setprecision(6);
    frames << "id,qpc_ms,received_qpc_ms\n" << std::fixed << std::setprecision(6);
    std::set<std::string> previous_markers;
    double previous_frame = -1.0;
    auto record_frame = [&](const capture::Direct3D11CaptureFrame& frame) {
        const double timestamp = frame.SystemRelativeTime().count() / 10000.0;
        const double observed_at = qpc_ms();
        if (timestamp < 0 || timestamp < previous_frame) {
            std::cerr << std::fixed << std::setprecision(6) << "frame_qpc_ms=" << timestamp
                      << " previous_qpc_ms=" << previous_frame << " observed_qpc_ms=" << observed_at << '\n';
            throw std::runtime_error("Invalid compositor QPC");
        }
        previous_frame = timestamp;
        frames << ',' << timestamp << ',' << observed_at << '\n';
        std::set<std::string> markers;
        read_grid(frame, [&](const auto& mapped) {
            for (UINT row = 0; row < grid.rows; ++row) {
                const auto id = recognize(grid.fingerprints(mapped, row), patterns);
                if (id.empty()) continue;
                // Record first appearances, spatial duplicates and reappearing IDs.
                if (!markers.insert(id).second || !previous_markers.contains(id)) {
                    frames << id << ',' << timestamp << ',' << observed_at << '\n';
                }
            }
        });
        previous_markers = std::move(markers);
    };
    record_frame(calibration);
    calibration.Close();

    if (!start_file.empty()) {
        std::ofstream trigger(start_file);
        trigger.exceptions(std::ios::failbit | std::ios::badbit);
        trigger << "start\n";
        trigger.close();
    }
    auto next_input = qpc_ms() + 750.0;
    UINT sample = 0;
    while (true) {
        foreground(hwnd, pid);
        double now = qpc_ms();
        if (now >= next_input) {
            if (sample == samples) break;
            const char key = 'a' + static_cast<char>(sample % 4);
            INPUT events[2]{};
            events[0].type = events[1].type = INPUT_KEYBOARD;
            events[0].ki.wVk = events[1].ki.wVk = static_cast<WORD>('A' + (key - 'a'));
            events[1].ki.dwFlags = KEYEVENTF_KEYUP;
            const double sent_at = qpc_ms();
            require(SendInput(2, events, sizeof(INPUT)) == 2, "SendInput failed or partially injected a key");
            inputs << token(++sample, key) << ',' << sent_at << '\n';
            next_input = sent_at + (sample == samples ? 1800.0 : interval_ms);
        }
        const auto wait_ms = static_cast<DWORD>(std::clamp(std::ceil(next_input - qpc_ms()), 0.0, 15.0));
        require(WaitForSingleObject(handle, wait_ms) != WAIT_FAILED, "Frame wait failed");
        while (auto frame = pool.TryGetNextFrame()) record_frame(frame);
    }
    session.Close();
    subscription.revoke();
    pool.Close();
    inputs.close();
    frames.close();
}

int wmain(int argc, wchar_t** argv) try {
    require(argc == 10 || argc == 11,
        "terminal-echo-windows HWND PID X Y CELL_W CELL_H ROWS SAMPLES INTERVAL_MS [START_FILE] (run from evidence directory)");
    const auto hwnd = reinterpret_cast<HWND>(std::stoull(argv[1], nullptr, 0));
    const auto raw_pid = std::stoul(argv[2]);
    Grid grid{static_cast<UINT>(std::stoul(argv[3])), static_cast<UINT>(std::stoul(argv[4])),
              static_cast<UINT>(std::stoul(argv[7])), std::stod(argv[5]), std::stod(argv[6])};
    const auto samples = std::stoul(argv[8]);
    const auto interval_ms = std::stoul(argv[9]);
    require(grid.cell_width >= 8 && grid.cell_width <= 100 && grid.cell_height >= 8 && grid.cell_height <= 100 &&
            grid.rows >= 6 && grid.rows <= 150 && samples >= 1 && samples <= 255 &&
            interval_ms >= 80 && interval_ms <= 3000, "Grid or sampling arguments out of range");
    winrt::init_apartment(winrt::apartment_type::multi_threaded);
    run(hwnd, raw_pid, grid, samples, interval_ms, std::filesystem::current_path(),
        argc == 11 ? std::filesystem::path(argv[10]) : std::filesystem::path());
    return 0;
} catch (const winrt::hresult_error& error) {
    std::wcerr << L"terminal-echo-windows: " << error.message().c_str() << L'\n';
    return 1;
} catch (const std::exception& error) {
    std::cerr << "terminal-echo-windows: " << error.what() << '\n';
    return 1;
}
