// A simple system context diagram
//^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ @comment
!include people.dsl
// <- @keyword.directive

workspace {
// <- @keyword
    model {
    // <- @keyword
        user = person "User"
        system = softwareSystem "System" {
            web = container "Web application" "TypeScript"
        }

        user -> system.web "Uses" "HTTPS"
    }

    views {
    // <- @keyword
        systemContext system {
            include *
        }
    }
}
