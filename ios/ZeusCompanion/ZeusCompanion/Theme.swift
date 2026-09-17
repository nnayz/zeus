import SwiftUI
import UIKit

// MARK: - Design Tokens & Visual Language
// Conforming to the ChatGPT iOS Mobile UI Design Specification:
// Minimal, editorial, content-first, spacious, quiet, and native to iOS.

enum Theme {

    // MARK: - Spacing Scale (4/8 px system)
    enum Spacing {
        static let xs: CGFloat = 4      // micro spacing
        static let sm: CGFloat = 8      // icon/text relationships
        static let md: CGFloat = 12     // compact controls
        static let lg: CGFloat = 16     // standard spacing / screen margins
        static let xl: CGFloat = 24     // content separation
        static let xxl: CGFloat = 32    // section separation
        static let xxxl: CGFloat = 48   // major visual separation
    }

    // MARK: - Corner Radii
    enum Radius {
        static let small: CGFloat = 8
        static let medium: CGFloat = 12
        static let large: CGFloat = 16
        static let composer: CGFloat = 22
        static let pill: CGFloat = 999
    }

    // MARK: - Colors
    enum Colors {
        // Base canvas: warm off-white in light mode, deep dark neutral in dark mode
        static var background: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.07, green: 0.07, blue: 0.08, alpha: 1.0) // #121214
                    : UIColor(red: 0.98, green: 0.98, blue: 0.97, alpha: 1.0) // #FAF9F7
            })
        }

        // Slightly elevated surface (cards, secondary containers)
        static var surface: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.11, green: 0.11, blue: 0.12, alpha: 1.0) // #1C1C1F
                    : UIColor(red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0)    // Pure white
            })
        }

        // Composer & interactive surfaces
        static var elevatedSurface: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.14, green: 0.14, blue: 0.16, alpha: 1.0) // #242429
                    : UIColor(red: 0.95, green: 0.95, blue: 0.94, alpha: 1.0) // #F2F2F0
            })
        }

        // Code block / terminal surface
        static var codeSurface: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.09, green: 0.09, blue: 0.10, alpha: 1.0) // #17171A
                    : UIColor(red: 0.94, green: 0.94, blue: 0.93, alpha: 1.0) // #F0F0EE
            })
        }

        // Subtle borders
        static var separator: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(white: 1.0, alpha: 0.07)
                    : UIColor(white: 0.0, alpha: 0.06)
            })
        }

        // Active / focused borders
        static var borderFocused: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(white: 1.0, alpha: 0.16)
                    : UIColor(white: 0.0, alpha: 0.14)
            })
        }

        // Typography colors: Dark charcoal rather than pure black
        static var primaryText: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.93, green: 0.93, blue: 0.94, alpha: 1.0) // #EDEDF0
                    : UIColor(red: 0.11, green: 0.11, blue: 0.13, alpha: 1.0) // #1C1C21
            })
        }

        static var secondaryText: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.60, green: 0.60, blue: 0.64, alpha: 1.0) // #9999A3
                    : UIColor(red: 0.44, green: 0.44, blue: 0.47, alpha: 1.0) // #707078
            })
        }

        static var tertiaryText: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor(red: 0.38, green: 0.38, blue: 0.42, alpha: 1.0)
                    : UIColor(red: 0.65, green: 0.65, blue: 0.68, alpha: 1.0)
            })
        }

        // Restrained accent: OpenAI / Editorial calm green
        static let accent = Color(red: 0.06, green: 0.64, blue: 0.50) // #10A37F
        static let accentMuted = Color(red: 0.06, green: 0.64, blue: 0.50).opacity(0.12)
        static let destructive = Color(red: 0.92, green: 0.26, blue: 0.26)

        // Send button background
        static var sendButtonBackground: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor.white
                    : UIColor(red: 0.11, green: 0.11, blue: 0.13, alpha: 1.0)
            })
        }

        static var sendButtonForeground: Color {
            Color(UIColor { trait in
                trait.userInterfaceStyle == .dark
                    ? UIColor.black
                    : UIColor.white
            })
        }
    }

    // MARK: - Typography System
    enum Typography {
        static let display = Font.system(size: 30, weight: .bold, design: .default)
        static let heading = Font.system(size: 22, weight: .semibold, design: .default)
        static let title = Font.system(size: 17, weight: .semibold, design: .default)
        static let bodyLarge = Font.system(size: 17, weight: .regular, design: .default)
        static let body = Font.system(size: 16, weight: .regular, design: .default)
        static let secondary = Font.system(size: 14, weight: .regular, design: .default)
        static let caption = Font.system(size: 12, weight: .medium, design: .default)
        static func mono(size: CGFloat = 13, weight: Font.Weight = .regular) -> Font {
            Font.system(size: size, weight: weight, design: .monospaced)
        }
    }
}

// MARK: - Tactile Haptic Engine
enum Haptic {
    static func light() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }
    static func medium() {
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
    }
    static func selection() {
        UISelectionFeedbackGenerator().selectionChanged()
    }
    static func success() {
        UINotificationFeedbackGenerator().notificationOccurred(.success)
    }
    static func error() {
        UINotificationFeedbackGenerator().notificationOccurred(.error)
    }
}

// MARK: - Minimal View Modifiers
struct EditorialCardModifier: ViewModifier {
    var padding: CGFloat = Theme.Spacing.lg
    var cornerRadius: CGFloat = Theme.Radius.medium

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background(Theme.Colors.surface)
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .stroke(Theme.Colors.separator, lineWidth: 1)
            )
    }
}

extension View {
    func editorialCard(padding: CGFloat = Theme.Spacing.lg, cornerRadius: CGFloat = Theme.Radius.medium) -> some View {
        modifier(EditorialCardModifier(padding: padding, cornerRadius: cornerRadius))
    }
}

// MARK: - Pulsing Subtle Status Indicator
struct SubtleStatusIndicator: View {
    let isActive: Bool

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(isActive ? Theme.Colors.accent : Color.secondary.opacity(0.4))
                .frame(width: 7, height: 7)
            Text(isActive ? "Live" : "Idle")
                .font(Theme.Typography.caption)
                .foregroundStyle(Theme.Colors.secondaryText)
        }
    }
}
