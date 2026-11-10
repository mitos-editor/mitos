workspace {
    model {
        user = person "User"
        system = softwareSystem "System" {
            web = container "Web application"
        }
    }

    views {
        systemContext system {
            include *
        }
    }
}
